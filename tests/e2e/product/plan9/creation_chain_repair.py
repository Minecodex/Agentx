"""Read-only workflow diagnostics and settled-write checks for cross-Agent acceptance."""

import ast
import json
import re
import uuid

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql


def schema_type_diagnostic(primary):
    """Keep only an allowlisted type clause, never the rejected business value."""
    if primary.get("code") != "CODE_OUTPUT_SCHEMA_VALIDATION_FAILED":
        return None
    match = re.search(
        r' is not of type "(integer|number|string|boolean|object|array|null)"$', primary.get("message", "")
    )
    return {"keyword": "type", "expectedType": match[1]} if match else None


def python_binding_diagnostic(node):
    parameters = node.get("parameters", {})
    if node.get("type") != "code" or parameters.get("runner") != "python":
        return None
    try:
        functions = [item for item in ast.parse(parameters.get("source", "")).body if isinstance(item, ast.FunctionDef)]
    except SyntaxError as error:
        return {"syntaxError": error.msg, "line": error.lineno}
    main = next((item for item in functions if item.name == "main"), None)
    if main is None:
        return {"missingMain": True}
    names = [item.arg for item in [*main.args.args, *main.args.kwonlyargs]]
    inputs = parameters.get("inputs", {}).get("fields", {})
    return {
        "boundKeywordNames": sorted(inputs),
        "namedMainParameters": names,
        "unmatchedBoundKeywords": sorted(set(inputs) - set(names)),
        "unboundNamedParameters": sorted(set(names) - set(inputs)),
        "hasCatchAllKeywords": main.args.kwarg is not None,
        "note": "Read-only AST diagnostic; input binding names are keyword arguments. A catch-all may swallow a misspelled name. Check actual upstream output selectors too; never invent a default receipt.",
    }


def known_runtime_failure(suite, task):
    """A new validation is allowed only after every existing tool Effect settled."""
    assert task["state"] in {"failed", "completed"}, "Unknown or running business operations cannot be replayed"
    identifier = uuid.UUID(task["taskId"]).hex
    rows = _runtime_mysql(
        suite["installed"],
        f"SELECT HEX(CAST(JSON_OBJECT('effectId',LOWER(HEX(f.id)),'settled',f.result_json IS NOT NULL,'unknown',f.outcome_unknown) AS BINARY)) FROM agent_tool_effects f JOIN application_invocations i ON i.tenant_id=f.tenant_id AND i.execution_id=f.execution_id WHERE i.id=UNHEX('{identifier}')",  # noqa: S608 -- parsed UUID only.
    )
    effects = [json.loads(bytes.fromhex(row)) for row in rows.splitlines() if row]
    assert all(row["settled"] and not row["unknown"] for row in effects), effects
    return {"taskId": task["taskId"], "state": task["state"], "effects": effects, "unknownWrites": False}


def repair_blueprint(suite, blueprint, generated, failure, directory, remaining):
    task = failure["task"]
    proof = known_runtime_failure(suite, task)
    workflow, version = generated["workflowId"], generated["publishedVersionId"]
    definition = get(suite["control"], f"/workflows/{workflow}/versions/{version}")["definition"]
    primary = (task.get("error") or {}).get("primaryError") or {}
    failed_node = next((node for node in definition["nodes"] if node["id"] == primary.get("sourceNodeId")), None)
    diagnostic = {
        "state": task["state"],
        "code": primary.get("code"),
        "sourceNodeId": primary.get("sourceNodeId"),
        "failedNode": failed_node,
        "pythonBinding": python_binding_diagnostic(failed_node) if failed_node else None,
        "schemaViolation": schema_type_diagnostic(primary),
        "businessValues": "Held-out test inputs, expected outputs and actual receipts are not provided.",
    }
    (directory / "workflow-runtime-failure.json").write_text(
        json.dumps(
            {"failure": failure, "settledEffects": proof, "diagnostic": diagnostic}, ensure_ascii=False, indent=2
        )
    )
    # Normal version-owned edit creates a mutable document; no graph/HTML patch.
    context = post(suite["control"], f"/workflows/{workflow}/versions/{version}/edit", {})
    return {
        **blueprint,
        "versionId": version,
        "draftId": context["document"]["id"],
        "attemptLimit": remaining,
        "prompt": (
            "修复当前工作流。下面是原需求和一次真实网页执行后的工作流诊断。网页提交的输入已逐字段验证正确，不能修改网页来规避。所有旧工具 Effect 已有确定收据；保留旧任务，不重放工具。通过画布工具修复工作流、保留必需节点 id/key、应用 Schema 和实际审批，检查所有 Code 入参名称与 main 参数精确对应，以及上游真实输出路径。Code 的 outputExample 决定输出类型；金额等允许小数的字段必须使用真正带小数的示例，0.0 经 JSON 往返仍是整数。禁止截断、四舍五入或强转真实业务金额来规避类型错误。禁止伪造、硬编码或用默认值替代凭证/结果，不运行、不发布、不提问。原业务需求："
            + blueprint["prompt"]
            + "。真实诊断（未提供验收业务值）："
            + json.dumps(diagnostic, ensure_ascii=False)
        ),
    }
