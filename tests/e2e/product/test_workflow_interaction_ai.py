"""Real model AI authoring followed by API Key, Hook and same-execution resumption."""

from __future__ import annotations

import json
import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import idempotent_headers, publish_fixture_version
from tests.e2e.product.studio_builder_support import _model
from tests.e2e.product.test_application_evaluations import read, send
from tests.e2e.product.test_channel_delivery import _mock_received, _publish_application
from tests.e2e.product.test_channel_delivery import im_mock as im_mock
from tests.e2e.product.test_live_memory_pipeline import _authorize_definition
from tests.e2e.product.test_studio_builder import capture_builder_evidence
from tests.e2e.product.test_workflow_interactions import poll
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def write(directory, name, value):
    (directory / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def test_real_ai_authored_workflow_api_hook_two_agent_branches(
    installed_agentx, service_urls, existing_model, existing_model_secret, im_mock, run_id
):
    directory = Path(installed_agentx["artifact_dir"]) / "interaction-ai"
    directory.mkdir(parents=True, exist_ok=True)
    write(directory, "connections.json", service_urls)
    hook_path = f"/input-hook/ai-{run_id}"
    hook_url = im_mock["url"] + hook_path
    with httpx.Client(base_url=service_urls["web"], timeout=180, trust_env=False) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        model = _model(
            control,
            me["departmentId"],
            run_id,
            "interaction-live",
            existing_model_secret.value,
            existing_model["endpoint"],
            existing_model["modelName"],
            {
                "currency": existing_model["currency"],
                "inputPerMillion": existing_model["inputPrice"],
                "outputPerMillion": existing_model["outputPrice"],
            },
            {key: existing_model[key] for key in ("defaultParameters", "maxInputTokens", "maxOutputTokens")},
        )
        prompt = f"""请直接在当前画布创建一个真实可运行的 API Key + Hook 语音确认工作流，不执行工作流，不向我提问。
主线：开始 → 需求分析智能体(intake) → 中间回复(progress) → 等待输入(ask) → 条件分支(route)。
收到原始文本“确认”时走确认执行智能体(approved)；任何其他文本走需求调整智能体(revise)。两支分别调用独立 Agent，最后汇入正常结束。每个有错误出口的执行节点都连接独立错误结束的 error 输入。
请先查询节点目录和实际参数示例，不要猜测节点类型。使用 agent v2、reply_message v1、await_input v1、if v1、exit v1。节点 id/key 分别使用 intake、progress、ask、route、approved、revise，所有 key 使用小写英文及下划线。
Start 输入是 object，仅必填 message:string，additionalProperties=false。最终正常输出仅 answer:string。正常 Exit 的 answer 使用 template，依次引用两个分支 Agent 的 outputs.text，两段 reference 都设置 missingPolicy.kind=omit；未经过分支会被忽略，只输出实际运行的那个 Agent 的文本。成功 Exit 不能引用 item。错误结束输出 code、message，使用当前 item 对应字段。
三个 Agent 都精确绑定真实模型别名 {model["alias"]} 的已发布部署版本，bindingRole=model；不绑定任何工具或沙箱，sessionPolicy.mode=invocation。
所有 Agent 预算：maxIterations=4,maxModelCalls=4,maxToolCalls=4,maxTotalTokens=24000,maxOutputTokens=1024,maxCost=1,maxDurationSeconds=180,limitAction=fail；节点 timeoutMs=240000；Workflow timeoutMs=900000。
需求分析智能体：userQuestion 引用 inputs.message。systemPrompt：你是需求分析助手，把用户需求整理成两条简洁建议，总计不超过100字，中文回答。
中间回复 content 用 template 拼接 intake 的 text，再追加“请回复：确认；或直接说出要修改的需求。”。它要继续下游，不作为最终答案。
等待输入参数只有 hookUrl，使用 InputBinding literal，value={json.dumps(hook_url)}。不设超时，不做意图识别。
条件分支配置一个 case id=confirmed，name=确认，logicalOp=and，conditions 中 condition.left 引用当前 item.content；operator=eq；right 是 literal 字符串“确认”。case:confirmed 连 approved，else 连 revise。不要比较整个输入对象。
确认执行智能体的 userQuestion 用 template 带入原始 inputs.message 和 ask 输出 content；systemPrompt：你是确认执行助手。必须以“确认分支：”开头，针对原始需求给出两步可执行建议，100字以内。
需求调整智能体的 userQuestion 同样带入原始需求和 ask 输出 content；systemPrompt：你是需求调整助手。必须以“调整分支：”开头，按照用户本次补充内容修改方案，100字以内，必须提到补充的具体限制。
保持两个 Agent 分支各自独立，节点之间使用正式 Binding，不用代码伪造智能体输出。两个分支 Agent 的 main 都直接连接正常结束，不增加合并节点或 Set Fields 节点。正常结束 answer 的精确绑定为 {{"kind":"template","segments":[{{"kind":"reference","selector":{{"namespace":"outputs","sourceNodeId":"approved","port":"main","path":["text"]}},"missingPolicy":{{"kind":"omit"}}}},{{"kind":"reference","selector":{{"namespace":"outputs","sourceNodeId":"revise","port":"main","path":["text"]}},"missingPolicy":{{"kind":"omit"}}}}]}}。Agent 输出就是 text，不存在 json.text 或 item.json，不要添加 json 路径层级。错误源 error 必须连 Exit.error。
完成连线、布局和完整静态校验。遇到资源待授权保留所选模型，我稍后通过正式授权入口补齐。不发布、不调试、不自动运行。"""
        (directory / "prompt.txt").write_text(prompt)
        environment = {
            **os.environ,
            "AGENTX_INTERACTION_AI_DIR": str(directory),
            "AGENTX_E2E_RUN_ID": run_id,
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
            "AGENTX_E2E_STUDIO_LIVE_ALIAS": model["alias"],
        }
        try:
            run_playwright(
                Path(installed_agentx["root"]),
                "interaction-ai-create",
                ("tests/workflow-interaction-ai.spec.ts",),
                environment,
            )
            created = json.loads((directory / "created.json").read_text())
            workflow = created["id"]
            document_id = created["documentId"]
            for attempt in range(3):
                draft = read(control, f"/workflows/{workflow}/drafts/{document_id}")
                _authorize_definition(control, workflow, draft["definition"])
                validation = send(
                    control,
                    f"/workflows/{workflow}/drafts/{document_id}/validate",
                    {key: draft[key] for key in ("definition", "editorDocument")},
                )
                issues = [issue for issue in validation["issues"] if issue["severity"] == "error"]
                route = next(node for node in draft["definition"]["nodes"] if node["type"] == "if")
                case = route["parameters"]["cases"][0]
                expected_port = "case:" + case["id"]
                if not any(
                    edge["sourceNodeId"] == route["id"] and edge["sourceHandle"] == expected_port
                    for edge in draft["definition"]["connections"]
                ):
                    issues.append(
                        {
                            "code": "DEMO_BRANCH_PORT_INVALID",
                            "nodeId": route["id"],
                            "message": f"确认分支的 sourceHandle 必须为 {expected_port}，不能为泛化的 case",
                        }
                    )
                write(directory, f"validation-{attempt + 1}.json", validation)
                if not issues or attempt == 2:
                    break
                (directory / "repair.txt").write_text(
                    "请只修复当前工作流的以下问题，保留三个智能体、回复、等待和条件分支，不执行不发布。"
                    "两个分支 Agent 直接连正常 Exit.main；正常 Exit.answer 使用 template，"
                    '包含两个 namespace=outputs、path=["text"] 的 reference，sourceNodeId 分别为两个分支 Agent 的实际 id，'
                    '每段 missingPolicy={"kind":"omit"}。不使用 item、json.text 或额外合并节点。'
                    "条件的确认出口使用 case:<cases[0].id>，不能用泛化 case；else 保持调整分支。"
                    "修复并保存，完成完整静态校验。具体问题：" + json.dumps(issues, ensure_ascii=False)
                )
                run_playwright(
                    Path(installed_agentx["root"]),
                    f"interaction-ai-repair-{attempt + 1}",
                    ("tests/workflow-interaction-ai.spec.ts",),
                    {**environment, "AGENTX_INTERACTION_AI_PHASE": "repair"},
                )
            write(directory, "validation.json", validation)
            assert not issues, issues
            write(directory, "ai-draft.json", draft)
            route = next(node for node in draft["definition"]["nodes"] if node["type"] == "if")
            cases = route["parameters"]["cases"]
            assert len(cases) == 1
            confirmed_port = "case:" + cases[0]["id"]
            assert cases[0]["conditions"][0]["condition"]["right"]["value"] == "确认"
            branches = {
                edge["sourceHandle"]: edge["targetNodeId"]
                for edge in draft["definition"]["connections"]
                if edge["sourceNodeId"] == route["id"]
            }
            assert branches[confirmed_port] != branches["else"]
            for node_id in (branches[confirmed_port], branches["else"]):
                assert next(node for node in draft["definition"]["nodes"] if node["id"] == node_id)["type"] == "agent"
            published = publish_fixture_version(control, workflow, document_id, draft["revision"])
            assert published.is_success, published.text
            version = published.json()
            app = send(
                control,
                "/applications",
                {
                    "name": "AI 语音确认 · Hook 分流闭环",
                    "slug": f"ai-input-{run_id}",
                    "workflowId": workflow,
                    "visibility": "company",
                },
            )
            development = next(value for value in read(control, "/environments") if value["code"] == "development")
            _publish_application(
                control,
                dict(control.headers),
                {"workflowId": workflow, "versionId": version["id"]},
                app["id"],
                development["id"],
            )
            key = send(control, f"/applications/{app['id']}/api-keys", {"name": "AI Hook closure demo"})
            write(
                directory,
                "published.json",
                {
                    "workflowId": workflow,
                    "workflowVersionId": version["id"],
                    "applicationId": app["id"],
                    "slug": app["slug"],
                    "apiKeyId": key["id"],
                    "modelAlias": model["alias"],
                },
            )
            with httpx.Client(
                base_url=service_urls["runtime"],
                timeout=180,
                trust_env=False,
                headers={"Authorization": f"Bearer {key['secret']}"},
            ) as gateway:
                exercise_api(control, gateway, app, branches, confirmed_port, installed_agentx, directory, hook_path)
            run_playwright(
                Path(installed_agentx["root"]),
                "interaction-ai-review",
                ("tests/workflow-interaction-ai.spec.ts",),
                {**environment, "AGENTX_INTERACTION_AI_PHASE": "review"},
            )
        finally:
            capture_builder_evidence(installed_agentx)


def exercise_api(control, gateway, app, branches, confirmed_port, installed_agentx, directory, hook_path):
    results = []
    for index, (reply, prefix, expected, excluded) in enumerate(
        [
            ("确认", "确认分支：", branches[confirmed_port], branches["else"]),
            ("先别执行，预算改成500元", "调整分支：", branches["else"], branches[confirmed_port]),
        ]
    ):
        invocation_headers = idempotent_headers()
        accepted_response = poll(
            lambda invocation_headers=invocation_headers: gateway.post(
                f"/gateway/v1/applications/{app['slug']}/invocations",
                headers=invocation_headers,
                json={"input": {"message": "帮我安排一个周末团队活动方案"}, "responseMode": "async"},
            ),
            lambda response: response.status_code != 401,
        )
        assert accepted_response.status_code in (200, 202), accepted_response.text
        accepted = accepted_response.json()
        path = f"/gateway/v1/invocations/{accepted['id']}"

        def query(path=path):
            response = gateway.get(path)
            response.raise_for_status()
            value = response.json()
            assert value["status"] not in ("failed", "cancelled"), value
            return value

        waiting = poll(query, lambda value: bool(value.get("activeWaits")), timeout=300)
        assert waiting["outputs"] is None
        assert len(waiting["messages"]) == 1
        assert "请回复" in waiting["messages"][0]["content"]
        wait = waiting["activeWaits"][0]
        notices = poll(
            lambda: _mock_received(installed_agentx["dependencies_namespace"], hook_path),
            lambda rows, wait=wait: any(row["body"].get("waitId") == wait["waitId"] for row in rows),
        )
        notice = next(row["body"] for row in notices if row["body"].get("waitId") == wait["waitId"])
        assert notice["invocationId"] == accepted["id"]
        assert notice["executionId"] == accepted["executionId"]
        assert notice["resumeUrl"] == wait["resumeUrl"]
        # The simulated customer receives the Hook, then fetches the already persisted message for TTS.
        customer_view = query()
        assert customer_view["activeWaits"][0]["waitId"] == wait["waitId"], "Hook ACK must not resume"
        callback_headers = {"Idempotency-Key": f"asr-reply-{index}"}
        callback = gateway.post(notice["resumeUrl"], headers=callback_headers, json={"content": reply})
        assert callback.status_code == 202, callback.text
        duplicate = gateway.post(notice["resumeUrl"], headers=callback_headers, json={"content": reply})
        assert duplicate.status_code == 202, duplicate.text
        assert duplicate.json()["receiptId"] == callback.json()["receiptId"]
        final = poll(query, lambda value: value["status"] == "completed", timeout=300)
        assert final["executionId"] == accepted["executionId"]
        assert final["outputs"]["answer"].startswith(prefix), final
        if index == 1:
            assert "500" in final["outputs"]["answer"], final
        assert final["activeWaits"] == []
        details = read(control, f"/executions/{accepted['executionId']}/runtime-details")
        write(directory, f"execution-{index + 1}-details.json", details)
        results.append(
            {
                "accepted": accepted,
                "waiting": waiting,
                "hook": notice,
                "customerMessage": customer_view["messages"][0],
                "asrReply": reply,
                "callback": callback.json(),
                "duplicateReceipt": duplicate.json(),
                "final": final,
                "expectedAgentId": expected,
                "excludedAgentId": excluded,
            }
        )
        write(directory, "api-results.json", results)
        print(
            f"Branch {index + 1}: API Key -> Agent -> message -> Hook -> ASR callback -> {expected} -> completed",
            flush=True,
        )
    return results
