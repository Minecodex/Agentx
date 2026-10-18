"""Design isolation and ledger checks against the same disposable system environment."""

from __future__ import annotations

import json
import os
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

import httpx

from tests.e2e.draft_fixtures import fixture_draft_id, idempotent_headers, publish_fixture_version
from tests.e2e.runtime.test_agent_attachments import _access_token, _control_mysql, _runtime_mysql
from tests.e2e.runtime.test_plugin_boundaries import _install
from tests.e2e.support import run, run_playwright
from tests.e2e.workflow_fixtures import set_chain_definition


def resolver_counterexamples(context, urls):
    root = Path(context["root"])
    run(("corepack", "pnpm", "--filter", "agentx-canvas-plugin-template", "build"), timeout=180)
    run(("corepack", "pnpm", "--filter", "agentx-canvas-plugin-template", "pack:plugin"), timeout=180)
    with ZipFile(root / "src/plugins/templates/canvas-plugin/dist/acme-json-mapper.agentx-plugin") as archive:
        template = {name: archive.read(name) for name in archive.namelist()}
    complete = "export function resolveDefinition(){return {status:'complete',outputSchema:{type:'object'}}}"
    cases = {
        "pure": complete,
        "large_source": complete + "\n/*" + "x" * 180_000 + "*/",
        "fresh_process": "let count=0;export function resolveDefinition(){return {status:'complete',outputSchema:{type:'object',properties:{counter:{type:'integer',const:++count}}}}}",
        "top_fetch": "await fetch('http://169.254.169.254/latest/meta-data/');" + complete,
        "dynamic_import": "const fs=await import('node:fs');fs.readFileSync('/etc/passwd');" + complete,
        # Ordinary stdout is redirected to diagnostics. Bypass that wrapper to
        # exercise the boundary's rejection of an actual host RPC frame.
        "host_bridge": "process.stdout._write(JSON.stringify({jsonrpc:'2.0',id:'escape',method:'host.resourceOperation',params:{}})+'\\n','utf8',()=>{});"
        + complete,
        "credential_file": "const fs=await import('node:fs');fs.readFileSync('/var/run/secrets/kubernetes.io/serviceaccount/token');"
        + complete,
        "business_request": "await fetch('http://runtime-gateway:8080/api/v1/executions',{method:'POST',body:'{}'});"
        + complete,
    }
    evidence = []
    with httpx.Client(base_url=urls["web"], timeout=90, trust_env=False) as client:
        token, me = _access_token(client)
        headers = {"Authorization": f"Bearer {token}"}
        created = client.post(
            "/api/v1/workflows", headers=headers, json={"name": "P8 resolve only", "visibility": "company"}
        )
        created.raise_for_status()
        workflow = created.json()
        # Other suites can leave a published scheduled application running.
        # Observe the exact design target while checking every escape attempt.
        workflow_id = uuid.UUID(workflow["id"])
        execution_count = f"SELECT COUNT(*) FROM workflow_executions WHERE workflow_id=UUID_TO_BIN('{workflow_id}')"  # noqa: S608 -- UUID is validated
        before = _runtime_mysql(context, execution_count)
        path = f"/api/v1/workflows/{workflow['id']}/drafts/{fixture_draft_id(client, workflow['id'], headers)}"
        draft = client.get(path, headers=headers).json()
        for label, source in cases.items():
            entries = dict(template)
            manifest = json.loads(entries["manifest.json"])
            manifest.update(packageId=f"acme/resolve-{label}", displayName=f"Resolve {label}", traceRenderers=[])
            node = json.loads(entries["nodes/json_mapper.json"])
            node.update(nodeType=f"acme.resolve_{label}", displayName=label, providers=[])
            node["parameterSchema"] = {"type": "object", "properties": {}, "additionalProperties": False}
            if label == "fresh_process":
                node["parameterSchema"]["properties"] = {"nonce": {"type": "string"}}
            node["uiSchema"] = {"order": [], "fields": {}, "canvas": {"role": "default"}}
            entries["manifest.json"] = json.dumps(manifest).encode()
            entries["nodes/json_mapper.json"] = json.dumps(node).encode()
            entries["runtime/entry.js"] = source.encode()
            package = Path(context["artifact_dir"]) / f"resolve-{label}.agentx-plugin"
            with ZipFile(package, "w", ZIP_DEFLATED) as archive:
                for name, content in sorted(entries.items()):
                    info = ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
                    info.compress_type = ZIP_DEFLATED
                    info.external_attr = 0o100644 << 16
                    archive.writestr(info, content)
            _install(client, headers, package)
            definition = json.loads(json.dumps(draft["definition"]))
            definition["nodes"].insert(
                0,
                {
                    "id": "resolve",
                    "key": "resolve",
                    "type": node["nodeType"],
                    "typeVersion": 1,
                    "name": label,
                    "parameters": {},
                    "resourceReferences": [],
                    "settings": {},
                },
            )
            if label == "fresh_process":
                definition["nodes"][0]["parameters"] = {"nonce": "first"}
                second = json.loads(json.dumps(definition["nodes"][0]))
                second.update(id="resolve2", key="resolve2", parameters={"nonce": "second"})
                definition["nodes"].insert(1, second)
            response = client.post(path + "/resolve-plugins", headers=headers, json={"definition": definition})
            evidence.append({"case": label, "httpStatus": response.status_code, "body": response.json()})
            (Path(context["artifact_dir"]) / "studio-resolver-isolation.json").write_text(
                json.dumps(evidence, indent=2), encoding="utf-8"
            )
            if response.status_code >= 500:
                diagnostic = _runtime_mysql(
                    context,
                    "SELECT JSON_OBJECT('operationId',BIN_TO_UUID(operation_id),'status',status,'hasSandbox',sandbox_id IS NOT NULL) FROM sandbox_design_leases",
                )
                (Path(context["artifact_dir"]) / "studio-resolver-leases.jsonl").write_text(
                    diagnostic, encoding="utf-8"
                )
            if label in {"pure", "large_source", "fresh_process"}:
                assert response.status_code == 200, response.text
                assert response.json()["nodes"], response.text
                if label == "fresh_process":
                    for node_id in ("resolve", "resolve2"):
                        assert response.json()["nodes"][node_id]["outputSchema"]["properties"]["counter"]["const"] == 1
            else:
                assert response.status_code >= 400, (label, response.text)
        # Saving a large graph exercises static resolution only; no invocation
        # or load generation is started by this functional regression.
        started = time.monotonic()
        try:
            saved = client.put(
                path,
                headers=headers,
                timeout=60,
                json={
                    "definition": set_chain_definition(draft["definition"], 200),
                    "editorDocument": draft["editorDocument"],
                    "expectedRevision": draft["revision"],
                },
            )
        except httpx.TimeoutException:
            diagnostic = _runtime_mysql(
                context,
                "SELECT JSON_OBJECT('operation',BIN_TO_UUID(operation_id),'session',BIN_TO_UUID(session_id),'status',status,'createdAt',created_at,'leaseUntil',lease_until) FROM sandbox_design_leases ORDER BY created_at",
            )
            (Path(context["artifact_dir"]) / "studio-resolver-timeout-leases.jsonl").write_text(
                diagnostic, encoding="utf-8"
            )
            raise
        assert saved.status_code == 200, saved.text
        assert len(saved.json()["definition"]["nodes"]) == 200
        session = json.loads(
            _runtime_mysql(
                context,
                f"SELECT JSON_OBJECT('status',p.status,'hasSandbox',p.sandbox_id IS NOT NULL,'operations',(SELECT COUNT(*) FROM sandbox_design_leases c WHERE c.session_id=p.operation_id AND c.operation_id<>p.operation_id),'completed',(SELECT COUNT(*) FROM sandbox_design_leases c WHERE c.session_id=p.operation_id AND c.operation_id<>p.operation_id AND c.status='completed')) FROM sandbox_design_leases p WHERE p.tenant_id=UUID_TO_BIN('{me['companyId']}') AND p.session_id=p.operation_id ORDER BY p.created_at DESC LIMIT 1",  # noqa: S608 -- fixture tenant UUID
            )
        )
        assert session == {"status": "completed", "hasSandbox": True, "operations": 199, "completed": 199}, session
        (Path(context["artifact_dir"]) / "studio-resolver-large-graph.json").write_text(
            json.dumps({"nodes": 200, "elapsedSeconds": time.monotonic() - started, "session": session}),
            encoding="utf-8",
        )
    assert _runtime_mysql(context, execution_count) == before
    assert (
        _runtime_mysql(
            context,
            "SELECT COUNT(*) FROM sandbox_design_leases WHERE status IN ('creating','executing','idle','cleanup_pending')",
        )
        == "0"
    )
    (Path(context["artifact_dir"]) / "studio-resolver-isolation.json").write_text(
        json.dumps(evidence, indent=2), encoding="utf-8"
    )


def ledger_and_retention(context, urls):
    # Epoch aging is fault injection in the isolated databases, not a production-data migration.
    query = "SELECT COUNT(*) FROM runtime_calls c WHERE c.source_kind='studio_builder' AND (c.execution_id IS NOT NULL OR c.attempt_id IS NOT NULL OR c.node_execution_id IS NOT NULL OR c.studio_model_binding_json IS NULL AND c.status<>'cancelled')"
    assert _runtime_mysql(context, query) == "0"
    query = "SELECT COUNT(*) FROM runtime_calls c WHERE c.source_kind='studio_builder' AND c.status='succeeded' AND (c.input_tokens=0 OR c.output_tokens=0 OR c.cost_micros=0 OR (SELECT COUNT(*) FROM quota_usage_ledger q WHERE q.tenant_id=c.tenant_id AND q.scope_type='studio_builder_model' AND q.scope_id=BIN_TO_UUID(c.id))<>2)"
    assert _runtime_mysql(context, query) == "0"
    assert (
        int(
            _runtime_mysql(
                context,
                "SELECT COUNT(*) FROM runtime_calls WHERE source_kind='studio_builder' AND status='succeeded' AND cost_micros>0",
            )
        )
        > 0
    )
    before = _runtime_mysql(context, "SELECT SUM(cost_micros) FROM runtime_calls WHERE source_kind='studio_builder'")
    unknown = _control_mysql(context, "SELECT COUNT(*) FROM studio_builder_runs WHERE status='outcome_unknown'")
    _control_mysql(context, "UPDATE studio_builder_sessions SET updated_at=UTC_TIMESTAMP()-INTERVAL 200 DAY")
    _control_mysql(
        context,
        "UPDATE studio_builder_runs SET completed_at=UTC_TIMESTAMP()-INTERVAL 200 DAY WHERE status IN ('completed','failed','cancelled')",
    )
    _runtime_mysql(
        context,
        "UPDATE runtime_calls SET ended_at=UTC_TIMESTAMP()-INTERVAL 200 DAY WHERE source_kind='studio_builder' AND status IN ('succeeded','failed','cancelled')",
    )
    with httpx.Client(base_url=urls["web"], timeout=60, trust_env=False) as client:
        token, _ = _access_token(client)
        response = client.post(
            "/api/v1/retention-runs",
            headers={"Authorization": f"Bearer {token}"},
            json={
                "dryRun": False,
                "artifactRetentionDays": 1,
                "traceRetentionDays": 1,
                "messageRetentionDays": 1,
                "evaluationRetentionDays": 1,
            },
        )
        response.raise_for_status()
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            count = _runtime_mysql(
                context,
                "SELECT COUNT(*) FROM runtime_calls WHERE source_kind='studio_builder' AND status='succeeded' AND request_json IS NOT NULL",
            )
            if count == "0":
                break
            time.sleep(1)
        assert count == "0", "Settled design payload retention did not run"
    assert _control_mysql(context, "SELECT COUNT(*) FROM studio_builder_runs WHERE status='outcome_unknown'") == unknown
    assert (
        _runtime_mysql(context, "SELECT SUM(cost_micros) FROM runtime_calls WHERE source_kind='studio_builder'")
        == before
    )
    assert (
        _runtime_mysql(
            context,
            "SELECT COUNT(*) FROM runtime_calls WHERE source_kind='studio_builder' AND status='succeeded' AND studio_model_binding_json IS NULL",
        )
        == "0"
    )
    (Path(context["artifact_dir"]) / "studio-ledger-retention.json").write_text(
        json.dumps({"billableCostMicros": before, "unknownRunsRetained": unknown, "settledBodiesExpired": True}),
        encoding="utf-8",
    )


def control_offline(context, urls, models):
    # A prior fault test replaced Gateway; establish the supervised tunnel again
    # with a read-only probe before sending the single business invocation.
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        try:
            probe = httpx.get(urls["runtime"] + "/health/live", timeout=2, trust_env=False)
            if probe.status_code == 200:
                break
        except httpx.TransportError:
            pass
        time.sleep(0.5)
    else:
        raise AssertionError("Runtime tunnel did not reconnect after the Gateway fault")
    with httpx.Client(base_url=urls["web"], timeout=60, trust_env=False) as control:
        token, _ = _access_token(control)
        control.headers.update({"Authorization": f"Bearer {token}"})
        # Include the existing business Agent Core, not just a static Workflow.
        from tests.e2e.product import remaining_support

        model_response = control.get(f"/api/v1/models/aliases/{models['fixture']['id']}")
        model_response.raise_for_status()
        model = model_response.json()
        model["credential"] = {"id": model["credentialId"]}
        agent_workflow = remaining_support.model_workflow(
            {"control": control, "runId": context["run_id"]}, model, "P8 business Agent offline"
        )
        agent_definition = remaining_support.agent_definition(control, agent_workflow["workflowId"], model)
        agent_node = next(node for node in agent_definition["nodes"] if node["type"] == "agent")
        agent_node["parameters"]["systemPrompt"] = {
            "kind": "template",
            "segments": [{"kind": "text", "text": "Answer the question using only the configured model."}],
        }
        agent_version = remaining_support.save_version(control, agent_workflow["workflowId"], agent_definition)
        setup_path = Path(context["artifact_dir"]) / "studio-offline-published-sources.json"
        run_playwright(
            Path(context["root"]),
            "studio-offline-setup",
            ("tests/studio-builder-offline-setup.spec.ts",),
            {
                **os.environ,
                "AGENTX_E2E_STAGE": "helm-agentxctl",
                "AGENTX_E2E_RUN_ID": context["run_id"],
                "AGENTX_E2E_BASE_URL": urls["web"],
                "AGENTX_E2E_RUNTIME_URL": urls["runtime"],
                "AGENTX_E2E_OFFLINE_AGENT_WORKFLOW": agent_workflow["workflowId"],
                "AGENTX_E2E_OFFLINE_AGENT_VERSION": agent_version["id"],
                "AGENTX_E2E_OFFLINE_SETUP_OUTPUT": str(setup_path),
            },
        )
        published_sources = json.loads(setup_path.read_text())
        workflow = {"id": published_sources["workflowId"]}
        deployments = control.get(f"/api/v1/workflows/{workflow['id']}/deployments").json()
        deployed = next(item for item in deployments if item["status"] == "active")
        created = control.post(
            "/api/v1/applications",
            json={
                "workflowId": workflow["id"],
                "name": "P8 offline fixture",
                "slug": "p8-offline",
                "visibility": "company",
            },
        )
        created.raise_for_status()
        application = created.json()
        key = control.post(f"/api/v1/applications/{application['id']}/api-keys", json={"name": "Isolated offline test"})
        key.raise_for_status()
        secret = key.json()["secret"]
        response = control.post(
            f"/api/v1/applications/{application['id']}/deployments",
            json={
                "workflowVersionId": deployed["workflowVersionId"],
                "environmentId": deployed["environmentId"],
                "sessionVersionPolicy": "pinned",
            },
        )
        response.raise_for_status()
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            items = control.get(f"/api/v1/applications/{application['id']}/deployments").json()
            if any(item["status"] == "active" for item in items):
                break
            time.sleep(1)
        else:
            raise AssertionError("Offline fixture application deployment did not activate")
        agent_application = control.post(
            "/api/v1/applications",
            json={
                "workflowId": agent_workflow["workflowId"],
                "name": "P8 offline business Agent",
                "slug": "p8-agent-offline",
                "visibility": "company",
            },
        )
        agent_application.raise_for_status()
        agent_app_id = agent_application.json()["id"]
        agent_key = control.post(
            f"/api/v1/applications/{agent_app_id}/api-keys", json={"name": "Isolated Agent offline test"}
        )
        agent_key.raise_for_status()
        agent_secret = agent_key.json()["secret"]
        agent_deployment = control.post(
            f"/api/v1/applications/{agent_app_id}/deployments",
            json={
                "workflowVersionId": agent_version["id"],
                "environmentId": deployed["environmentId"],
                "sessionVersionPolicy": "pinned",
            },
        )
        agent_deployment.raise_for_status()
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if any(
                item["status"] == "active"
                for item in control.get(f"/api/v1/applications/{agent_app_id}/deployments").json()
            ):
                break
            time.sleep(1)
        else:
            raise AssertionError("Business Agent offline deployment did not activate")
        namespace = context["control_namespace"]
        try:
            run(("kubectl", "-n", namespace, "scale", "deployment/platform-control", "--replicas=0"))
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "wait",
                    "--for=delete",
                    "pod",
                    "-l",
                    "app.kubernetes.io/name=platform-control",
                    "--timeout=60s",
                ),
                timeout=70,
            )
            try:
                unavailable = control.get("/api/v1/platform-ai/model-setting", timeout=3)
                unavailable_status = unavailable.status_code
                assert unavailable_status >= 500
            except httpx.TransportError:
                unavailable_status = "unreachable"
            with httpx.Client(
                base_url=urls["runtime"], timeout=60, trust_env=False, headers={"Authorization": f"Bearer {secret}"}
            ) as runtime:
                invocation = runtime.post(
                    "/gateway/v1/applications/p8-offline/invocations",
                    headers={"Idempotency-Key": str(uuid.uuid4())},
                    json={"input": {}, "responseMode": "async"},
                )
                assert invocation.status_code == 202, invocation.text
                identity = invocation.json()["id"]
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    receipt = runtime.get(f"/gateway/v1/invocations/{identity}")
                    receipt.raise_for_status()
                    if receipt.json()["status"] in {"completed", "failed", "cancelled"}:
                        break
                    time.sleep(0.5)
                assert receipt.json()["status"] == "completed", receipt.text
                agent_invocation = runtime.post(
                    "/gateway/v1/applications/p8-agent-offline/invocations",
                    headers={"Authorization": f"Bearer {agent_secret}", "Idempotency-Key": str(uuid.uuid4())},
                    json={"input": {"message": "P8 offline business Agent"}, "responseMode": "async"},
                )
                assert agent_invocation.status_code == 202, agent_invocation.text
                agent_id = agent_invocation.json()["id"]
                deadline = time.monotonic() + 90
                while time.monotonic() < deadline:
                    agent_receipt = runtime.get(
                        f"/gateway/v1/invocations/{agent_id}", headers={"Authorization": f"Bearer {agent_secret}"}
                    )
                    agent_receipt.raise_for_status()
                    if agent_receipt.json()["status"] in {"completed", "failed", "cancelled"}:
                        break
                    time.sleep(0.5)
                assert agent_receipt.json()["status"] == "completed", agent_receipt.text
                agent_execution = str(uuid.UUID(agent_receipt.json()["executionId"]))
                calls = _runtime_mysql(
                    context,
                    f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{agent_execution}') AND source_kind='workflow' AND agent_run_id IS NOT NULL AND call_kind='model' AND status='succeeded';",  # noqa: S608 -- parsed execution UUID.
                )
                assert int(calls) >= 1, "A real business Agent model call must settle with Control offline"
                (Path(context["artifact_dir"]) / "studio-control-offline.json").write_text(
                    json.dumps(
                        {
                            "builderHttpStatus": unavailable_status,
                            "invocationId": identity,
                            "status": "completed",
                            "workflowVersionId": deployed["workflowVersionId"],
                            "agentInvocationId": agent_id,
                            "agentWorkflowVersionId": agent_version["id"],
                            "agentStatus": "completed",
                            "agentModelCalls": int(calls),
                        }
                    ),
                    encoding="utf-8",
                )
        finally:
            run(("kubectl", "-n", namespace, "scale", "deployment/platform-control", "--replicas=1"))
            run(
                ("kubectl", "-n", namespace, "rollout", "status", "deployment/platform-control", "--timeout=120s"),
                timeout=130,
            )


def protect_unsettled_sources(context, urls, models):
    with httpx.Client(base_url=urls["web"], timeout=60, trust_env=False) as client:
        token, _ = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        setting = client.get("/api/v1/platform-ai/model-setting").json()
        clear = client.put(
            "/api/v1/platform-ai/model-setting", json={"expectedVersion": setting["version"], "modelAliasId": None}
        )

        clear.raise_for_status()
        model = models["fixture"]["id"]
        impact = client.get(f"/api/v1/deletion-impact/model/{model}")
        impact.raise_for_status()
        assert any(item["sourceType"] == "studio_builder_run" for item in impact.json()["references"]), impact.text
        blocked = client.delete(f"/api/v1/models/aliases/{model}")
        assert blocked.status_code == 409, blocked.text
        reset = client.put(
            "/api/v1/platform-ai/model-setting",
            json={"expectedVersion": clear.json()["version"], "modelAliasId": model},
        )
        reset.raise_for_status()
        (Path(context["artifact_dir"]) / "studio-unsettled-delete-protection.json").write_text(
            json.dumps(
                {
                    "settingCleared": True,
                    "modelDeletionStatus": blocked.status_code,
                    "references": impact.json()["references"],
                }
            ),
            encoding="utf-8",
        )


def revision_delete_race(context, urls, models):
    """Fault the two competing commits; no browser/Editor state is injected."""
    evidence = []
    with httpx.Client(base_url=urls["web"], timeout=60, trust_env=False) as client:
        token, _ = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        source = client.get(f"/api/v1/models/aliases/{models['fixture']['id']}").json()
        prices = client.get(f"/api/v1/models/deployments/{source['deploymentId']}/prices").json()
        keys = (
            "connectionName",
            "providerType",
            "endpoint",
            "credentialId",
            "ownerDepartmentId",
            "modelName",
            "maxInputTokens",
            "maxOutputTokens",
            "defaultParameters",
            "capabilities",
        )
        for index in range(4):
            model = client.post(
                "/api/v1/models/aliases",
                json={
                    **{key: source[key] for key in keys},
                    "connectionName": f"P8 revision deletion {index}",
                    "alias": f"p8-revision-delete-{index}",
                    "price": {key: prices[0][key] for key in ("currency", "inputPerMillion", "outputPerMillion")},
                },
            )
            model.raise_for_status()
            model_id = model.json()["id"]
            created = client.post(
                "/api/v1/workflows", json={"name": f"P8 deletion race {index}", "visibility": "company"}
            ).json()
            path = f"/api/v1/workflows/{created['id']}/drafts/{fixture_draft_id(client, created['id'], None)}"
            draft = client.get(path).json()
            definition = draft["definition"]
            definition["nodes"].insert(
                0,
                {
                    "id": "reference",
                    "key": "reference",
                    "type": "if",
                    "typeVersion": 1,
                    "name": "Reference",
                    "parameters": {
                        "cases": [
                            {
                                "id": "yes",
                                "name": "Yes",
                                "logicalOp": "and",
                                "conditions": [
                                    {
                                        "condition": {
                                            "left": {"kind": "literal", "value": True},
                                            "operator": "eq",
                                            "right": {"kind": "literal", "value": True},
                                        }
                                    }
                                ],
                            }
                        ]
                    },
                    "resourceReferences": [{"resourceType": "model", "resourceId": model_id, "operation": "use"}],
                    "settings": {},
                },
            )
            with ThreadPoolExecutor(max_workers=2) as pool:
                save = pool.submit(
                    client.put,
                    path,
                    json={"expectedRevision": 0, "definition": definition, "editorDocument": draft["editorDocument"]},
                )
                deletion = pool.submit(client.delete, f"/api/v1/models/aliases/{model_id}")
                saved, deleted = save.result(), deletion.result()
            assert (saved.status_code == 200) != (deleted.status_code == 204), (saved.text, deleted.text)
            head = client.get(path).json()
            if saved.status_code == 200:
                assert deleted.status_code == 409 and head["revision"] == 1
                assert client.get(f"/api/v1/models/aliases/{model_id}").status_code == 200
            else:
                assert saved.status_code in (404, 409, 422) and head["revision"] == 0, saved.text
            evidence.append(
                {"saveStatus": saved.status_code, "deleteStatus": deleted.status_code, "revision": head["revision"]}
            )
            client.delete(f"/api/v1/workflows/{created['id']}").raise_for_status()
            if deleted.status_code != 204:
                client.delete(f"/api/v1/models/aliases/{model_id}").raise_for_status()
    (Path(context["artifact_dir"]) / "studio-revision-delete-race.json").write_text(
        json.dumps(evidence, indent=2), encoding="utf-8"
    )


def resource_request_isolation(context, urls, models):
    with httpx.Client(base_url=urls["web"], timeout=60, trust_env=False) as client:
        token, _ = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        created = client.post("/api/v1/workflows", json={"name": "P8 resource isolation", "visibility": "company"})
        created.raise_for_status()
        workflow = created.json()
        root = f"/api/v1/workflows/{workflow['id']}"
        second = client.post(root + "/versions", headers=idempotent_headers())
        second.raise_for_status()
        second_context = client.post(root + f"/versions/{second.json()['id']}/edit").json()
        draft_ids = [fixture_draft_id(client, workflow["id"]), second_context["document"]["id"]]
        model = models["alternate"]["id"]

        def option(draft):
            response = client.get(
                root + "/resource-options", params={"draftId": draft, "resourceType": "model", "operation": "use"}
            )
            response.raise_for_status()
            return next(item for item in response.json()["items"] if item["id"] == model)

        requests = []
        for index, draft in enumerate(draft_ids):
            assert option(draft)["pendingRequestId"] is None
            response = client.post(
                root + "/resource-grant-requests",
                json={
                    "resourceType": "model",
                    "resourceId": model,
                    "resourceVersionId": None,
                    "operation": "use",
                    "sourceDraftId": draft,
                    "sourceRevision": 0,
                    "sourceNodeId": None,
                    "message": "Isolated test request",
                },
            )
            response.raise_for_status()
            requests.append(response.json()["id"])
            assert option(draft)["pendingRequestId"] == requests[-1]
            if index == 0:
                assert option(draft_ids[1])["pendingRequestId"] is None
        assert len(set(requests)) == 2
        assert [option(draft)["pendingRequestId"] for draft in draft_ids] == requests
        foreign = client.post(
            "/api/v1/workflows", json={"name": "P8 foreign resources", "visibility": "company"}
        ).json()
        denied = client.get(
            root + "/resource-options",
            params={"draftId": fixture_draft_id(client, foreign["id"], None), "resourceType": "model"},
        )
        assert denied.status_code == 404, denied.text
        granted = client.post(
            root + "/resource-authorizations",
            json={"resourceType": "model", "resourceId": model, "resourceVersionId": None, "operation": "use"},
        )
        granted.raise_for_status()
        assert all(option(draft)["accessState"] == "authorized" for draft in draft_ids)
        for identity in requests:
            receipt = client.get(f"/api/v1/resource-grant-requests/{identity}").json()
            assert receipt["status"] == "approved" and receipt["sourceDraftId"] in draft_ids
        evidence = {
            "draftIds": draft_ids,
            "requestIds": requests,
            "crossWorkflowStatus": denied.status_code,
            "authorizationStatus": granted.status_code,
        }
    (Path(context["artifact_dir"]) / "studio-resource-request-isolation.json").write_text(
        json.dumps(evidence, indent=2), encoding="utf-8"
    )


def tenant_boundary_isolation(context, urls):
    """Seed foreign objects only in the disposable database, then use the real API."""
    identifiers = {key: str(uuid.uuid4()) for key in ("tenant", "user", "workflow", "draft", "revision", "version")}
    tenant, user, workflow, draft, revision, version = identifiers.values()
    with httpx.Client(base_url=urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers.update({"Authorization": f"Bearer {token}"})
        created = control.post(
            "/api/v1/workflows", json={"name": f"P8 tenant boundary {draft}", "visibility": "company"}
        )
        created.raise_for_status()
        own = created.json()
        own_draft = control.get(
            f"/api/v1/workflows/{own['id']}/drafts/{fixture_draft_id(control, own['id'], None)}"
        ).json()
        published = publish_fixture_version(control, own["id"], fixture_draft_id(control, own["id"], None), 0, None)
        published.raise_for_status()
        own_workflow = str(uuid.UUID(own["id"]))
        own_draft_id = str(uuid.UUID(fixture_draft_id(control, own["id"], None)))
        own_version = str(uuid.UUID(published.json()["id"]))
        own_user = str(uuid.UUID(me["id"]))
        # UUIDs are generated/parsed above; no credentials or arbitrary SQL enter this fixture.
        seeded = _control_mysql(
            context,
            f"""START TRANSACTION;
INSERT INTO tenants(id,name,normalized_name) VALUES(UUID_TO_BIN('{tenant}'),'P8 foreign tenant','p8_foreign_{tenant}');
INSERT INTO users(id,tenant_id,username,username_normalized,display_name)
SELECT UUID_TO_BIN('{user}'),UUID_TO_BIN('{tenant}'),'foreign','foreign','Foreign fixture' FROM users WHERE id=UUID_TO_BIN('{own_user}');
INSERT INTO workflows(id,tenant_id,name,visibility,owner_user_id,owner_department_id)
SELECT UUID_TO_BIN('{workflow}'),UUID_TO_BIN('{tenant}'),'Foreign Workflow','company',UUID_TO_BIN('{user}'),owner_department_id FROM workflows WHERE id=UUID_TO_BIN('{own_workflow}');
INSERT INTO workflow_drafts(id,tenant_id,workflow_id,created_by,updated_by,schema_version,revision,definition_json,content_hash,editor_json,editor_hash)
SELECT UUID_TO_BIN('{draft}'),UUID_TO_BIN('{tenant}'),UUID_TO_BIN('{workflow}'),UUID_TO_BIN('{user}'),UUID_TO_BIN('{user}'),schema_version,0,definition_json,content_hash,editor_json,editor_hash FROM workflow_drafts WHERE id=UUID_TO_BIN('{own_draft_id}');
INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,resource_snapshot_json,content_hash,created_by,editor_json,editor_hash)
SELECT UUID_TO_BIN('{revision}'),UUID_TO_BIN('{tenant}'),UUID_TO_BIN('{workflow}'),UUID_TO_BIN('{draft}'),revision,schema_version,definition_json,resource_snapshot_json,content_hash,UUID_TO_BIN('{user}'),editor_json,editor_hash FROM workflow_draft_revisions WHERE draft_id=UUID_TO_BIN('{own_draft_id}') AND revision=0;
INSERT INTO workflow_versions(id,tenant_id,workflow_id,source_draft_id,source_revision_id,version_number,source_revision,schema_version,definition_json,node_manifests_json,content_hash,created_by,editor_json,editor_hash,updated_by,publication_status,working_draft_id)
SELECT UUID_TO_BIN('{version}'),UUID_TO_BIN('{tenant}'),UUID_TO_BIN('{workflow}'),UUID_TO_BIN('{draft}'),UUID_TO_BIN('{revision}'),1,0,schema_version,definition_json,node_manifests_json,content_hash,UUID_TO_BIN('{user}'),editor_json,editor_hash,UUID_TO_BIN('{user}'),'published',UUID_TO_BIN('{draft}') FROM workflow_versions WHERE id=UUID_TO_BIN('{own_version}');
COMMIT;
SELECT COUNT(*) FROM workflow_versions WHERE tenant_id=UUID_TO_BIN('{tenant}') AND id=UUID_TO_BIN('{version}');""",  # noqa: S608 -- validated UUIDs in isolated test data only.
        )
        assert seeded.strip() == "1", "The foreign Version fixture must exist"
        snapshot = {
            "editorInstanceId": str(uuid.uuid4()),
            "localDocumentRevision": 0,
            "documentHash": "sha256:v1:" + "0" * 64,
            "serverRevision": 0,
            "saveStatus": "saved",
            "document": {"definition": own_draft["definition"], "editorDocument": own_draft["editorDocument"]},
            "selectedNodeIds": [],
            "selectedExecutionId": None,
        }
        statuses = {}
        for name, method, path, body in [
            ("workflow_read", "GET", f"/workflows/{workflow}", None),
            ("draft_read", "GET", f"/workflows/{workflow}/drafts/{draft}", None),
            ("own_workflow_foreign_draft", "GET", f"/workflows/{own_workflow}/drafts/{draft}", None),
            (
                "draft_save",
                "PUT",
                f"/workflows/{workflow}/drafts/{draft}",
                {
                    "expectedRevision": 0,
                    "definition": own_draft["definition"],
                    "editorDocument": own_draft["editorDocument"],
                },
            ),
            ("version_read", "GET", f"/workflows/{workflow}/versions/{version}", None),
            (
                "version_derive",
                "POST",
                f"/workflows/{own_workflow}/versions/{version}/edit",
                {},
            ),
            ("version_execute", "POST", f"/workflow-versions/{version}/executions", {"input": {}}),
            (
                "builder_create",
                "POST",
                f"/workflows/{workflow}/drafts/{draft}/builder-sessions",
                {"snapshot": snapshot},
            ),
            (
                "own_builder_foreign_draft",
                "POST",
                f"/workflows/{own_workflow}/drafts/{draft}/builder-sessions",
                {"snapshot": snapshot},
            ),
        ]:
            response = control.request(method, "/api/v1" + path, json=body)
            assert response.status_code == 404, (name, response.status_code, response.text)
            statuses[name] = response.status_code
        unchanged = control.get(f"/api/v1/workflows/{own_workflow}/drafts/{own_draft_id}")
        assert unchanged.status_code == 200 and unchanged.json()["revision"] == own_draft["revision"] + 1, (
            unchanged.text
        )
        assert (
            _control_mysql(context, f"SELECT revision FROM workflow_drafts WHERE id=UUID_TO_BIN('{draft}');").strip()  # noqa: S608 -- generated UUID.
            == "0"
        )
    (Path(context["artifact_dir"]) / "studio-tenant-boundaries.json").write_text(
        json.dumps({"foreignObjects": identifiers, "statuses": statuses, "foreignRevision": 0}, indent=2),
        encoding="utf-8",
    )
