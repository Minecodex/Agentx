from __future__ import annotations

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

import httpx
import pytest

from tests.e2e.support import MYSQL_ROOT_CLIENT, run


def _login(client: httpx.Client) -> str:
    bootstrap = client.get("/api/v1/bootstrap/status")
    bootstrap.raise_for_status()
    response = client.post(
        "/api/v1/bootstrap" if bootstrap.json()["required"] else "/api/v1/auth/login",
        json={
            "companyName": "Agentx E2E",
            "adminUsername": "admin",
            "adminDisplayName": "Agentx E2E Admin",
            "password": "agentx-e2e-admin-password",
            "locale": "zh-CN",
            "timezone": "Asia/Shanghai",
        }
        if bootstrap.json()["required"]
        else {"username": "admin", "password": "agentx-e2e-admin-password"},
    )
    response.raise_for_status()
    return str(response.json()["accessToken"])


def _fault_plugin(root: Path, directory: Path) -> Path:
    pnpm = ("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")
    subprocess.run(
        (*pnpm, "--filter", "agentx-canvas-plugin-template", "build"), cwd=root, check=True, shell=False, timeout=180
    )
    subprocess.run(
        (*pnpm, "--filter", "agentx-canvas-plugin-template", "pack:plugin"),
        cwd=root,
        check=True,
        shell=False,
        timeout=180,
    )
    source = root / "src" / "plugins" / "templates" / "canvas-plugin" / "dist" / "acme-json-mapper.agentx-plugin"
    with ZipFile(source) as archive:
        entries = {name: archive.read(name) for name in archive.namelist()}
    manifest = json.loads(entries["manifest.json"])
    manifest.update(
        packageId="acme/faults",
        packageVersion="1.0.0",
        displayName="Fault Plugin",
        uiEntry=None,
        uiStylesEntry=None,
        uiAssets={},
        traceRenderers=[],
    )
    node = json.loads(entries["nodes/json_mapper.json"])
    node.update(nodeType="acme.fault", displayName="Fault Plugin")
    node["parameterSchema"] = {
        "type": "object",
        "required": ["mode"],
        "properties": {
            "mode": {"type": "string", "enum": ["background", "crash", "hang", "sync_hang", "orphan", "recover"]}
        },
        "additionalProperties": False,
    }
    node["uiSchema"] = {"order": ["mode"], "fields": {"mode": {"control": "select"}}, "canvas": {"role": "default"}}
    entries["manifest.json"] = json.dumps(manifest, separators=(",", ":")).encode()
    entries["nodes/json_mapper.json"] = json.dumps(node, separators=(",", ":")).encode()
    entries["runtime/entry.js"] = (
        b"export function resolveDefinition(){return {status:'complete'}};export async function execute(ctx){const mode=ctx.parameters.mode;if(mode==='crash')process.exit(17);if(mode==='hang')await new Promise(()=>{});if(mode==='sync_hang')while(true){};if(mode==='recover')await new Promise(resolve=>setTimeout(resolve,5000));if(mode==='orphan'){const {spawn}=await import('node:child_process');spawn(process.execPath,['-e','process.title=\"agentx-plugin-orphan\";setInterval(()=>{},10000)'],{stdio:'ignore'});await new Promise(()=>{})}if(mode==='background')setInterval(()=>{},10000);return {status:'completed',outputs:{main:[{json:{mode}}]}}}"
    )
    target = directory / "faults.agentx-plugin"
    with ZipFile(target, "w", ZIP_DEFLATED) as archive:
        for name, content in sorted(entries.items()):
            info = ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)
    return target


def _wait_admission_delivery(installed_agentx: dict[str, str], timeout: float = 60) -> None:
    query = (
        "SELECT COUNT(*) FROM outbox WHERE aggregate_type IN "
        "('application_admission','runtime_user_admission','workflow_admission','application_chat_mapping') "
        "AND status IN ('pending','processing','failed');"
    )
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        pending = run(
            (
                "kubectl",
                "-n",
                installed_agentx["control_namespace"],
                "exec",
                "statefulset/control-mysql",
                "--",
                "sh",
                "-ec",
                f'{MYSQL_ROOT_CLIENT} --batch --skip-column-names -uroot agentx_control -e "$1"',
                "agentx-plugin-admission-query",
                query,
            ),
            timeout=30,
        ).stdout.strip()
        if pending == "0":
            return
        time.sleep(0.25)
    raise AssertionError(f"Canvas Plugin admission delivery did not settle: {pending}")


def _start_fault(client: httpx.Client, token: str, mode: str, installed_agentx: dict[str, str]) -> str:
    headers = {"Authorization": f"Bearer {token}"}
    created = client.post(
        "/api/v1/workflows", headers=headers, json={"name": f"Plugin {mode} {time.time_ns()}", "visibility": "company"}
    )
    created.raise_for_status()
    workflow_id = created.json()["id"]
    draft = client.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers).json()
    definition = draft["definition"]
    definition["nodes"] = [
        {
            "id": "fault",
            "key": "fault",
            "type": "acme.fault",
            "typeVersion": 1,
            "name": "Fault",
            "disabled": False,
            "protected": False,
            "parameters": {"mode": mode},
            "contextWrites": [],
            "resourceReferences": [],
            "settings": {"retryOnFail": False, "maxTries": 1, "timeoutMs": 20000 if mode == "recover" else 1200},
        },
        *definition["nodes"],
    ]
    definition["connections"] = [
        {
            "id": "start-fault",
            "sourceNodeId": "__start__",
            "sourceHandle": "main",
            "targetNodeId": "fault",
            "targetHandle": "main",
            "order": 0,
        },
        {
            "id": "fault-exit",
            "sourceNodeId": "fault",
            "sourceHandle": "main",
            "targetNodeId": "exit",
            "targetHandle": "main",
            "order": 0,
        },
    ]
    editor = draft["editorDocument"]
    editor["nodeLayouts"] = [{"nodeId": "fault", "x": 320, "y": 200}, *editor.get("nodeLayouts", [])]
    editor["edges"] = [{"edgeId": "start-fault"}, {"edgeId": "fault-exit"}]
    saved = client.put(
        f"/api/v1/workflows/{workflow_id}/draft",
        headers={**headers, "Idempotency-Key": str(time.time_ns())},
        json={"expectedRevision": draft["revision"], "definition": definition, "editorDocument": editor},
    )
    saved.raise_for_status()
    saved_revision = saved.json()["revision"]
    _wait_admission_delivery(installed_agentx)
    started = client.post(
        f"/api/v1/workflows/{workflow_id}/debug-executions",
        headers=headers,
        json={
            "expectedRevision": saved_revision,
            "idempotencyKey": f"fault-{mode}-{time.time_ns()}",
            "input": {},
            "context": {},
            "mode": "full",
            "targetNodeId": None,
            "inputSource": {},
            "overlayIds": [],
            "sideEffectDecisions": {},
        },
    )
    assert started.is_success, started.text
    return str(started.json()["executionId"])


def _wait_execution(client: httpx.Client, token: str, execution_id: str, timeout: float = 60) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        response = client.get(f"/api/v1/executions/{execution_id}", headers={"Authorization": f"Bearer {token}"})
        if response.status_code in {403, 404}:
            time.sleep(0.25)
            continue
        response.raise_for_status()
        value = response.json()
        if value["status"] in {"succeeded", "failed", "cancelled"}:
            return value
        time.sleep(0.25)
    raise AssertionError(f"Plugin execution {execution_id} did not finish")


@pytest.mark.cluster
@pytest.mark.runtime
def test_plugin_runner_image_and_handshake(installed_agentx: dict[str, str]) -> None:
    namespace = installed_agentx["runtime_namespace"]
    request = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": "e2e-initialize",
            "method": "runner.initialize",
            "params": {"protocolVersion": 2, "sdkApiVersion": 2},
        },
        separators=(",", ":"),
    )
    result = run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/workflow-worker",
            "--",
            "sh",
            "-ec",
            'test "$(node --version)" = "v24.20.0"; '
            "test -r /opt/agentx/plugin-runner/runner.mjs; "
            'test "$AGENTX_PLUGIN_MAX_PROCESSES" = "8"; '
            'test -w "$AGENTX_PLUGIN_WORK_DIR"; '
            'test "$AGENTX_PLUGIN_MEMORY_MB" = "96"; '
            'test "$(cat /proc/1/comm)" = "tini"; '
            "! env | grep '^AGENTX_CONTROL_'; "
            'printf "%s\\n" "$1" | node /opt/agentx/plugin-runner/runner.mjs',
            "agentx-plugin-e2e",
            request,
        ),
        timeout=60,
    )
    response = json.loads(result.stdout.strip().splitlines()[-1])
    assert response == {
        "jsonrpc": "2.0",
        "id": "e2e-initialize",
        "result": {
            "protocolVersion": 2,
            "sdkApiVersion": 2,
            "nodeVersion": "24.20.0",
        },
    }


@pytest.mark.cluster
@pytest.mark.runtime
def test_plugin_worker_contains_crash_timeout_background_and_process_tree(
    installed_agentx: dict[str, str], service_urls: dict[str, str]
) -> None:
    with tempfile.TemporaryDirectory(prefix="agentx-plugin-faults-") as directory:
        package = _fault_plugin(Path(installed_agentx["root"]), Path(directory))
        with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
            token = _login(client)
            headers = {"Authorization": f"Bearer {token}"}
            with package.open("rb") as stream:
                imported = client.post(
                    "/api/v1/canvas-plugin-imports",
                    headers=headers,
                    files={"file": (package.name, stream, "application/zip")},
                )
            imported.raise_for_status()
            preview = imported.json()
            installed = client.post(
                f"/api/v1/canvas-plugin-imports/{preview['id']}/install",
                headers=headers,
                json={"bundleDigest": preview["bundleDigest"], "enable": True, "setDefault": True},
            )
            installed.raise_for_status()
            background_id = _start_fault(client, token, "background", installed_agentx)
            background = _wait_execution(client, token, background_id)
            assert background["status"] == "succeeded", background
            trace_deadline = time.monotonic() + 30
            while time.monotonic() < trace_deadline:
                trace_response = client.get(f"/api/v1/executions/{background_id}/trace?limit=100", headers=headers)
                if trace_response.is_success:
                    span_kinds = {span["spanKind"] for span in trace_response.json()["spans"]}
                    if {"node", "attempt"}.issubset(span_kinds):
                        break
                time.sleep(0.25)
            else:
                raise AssertionError("zero-instrumentation plugin has no platform Node/Attempt Trace")
            namespace = installed_agentx["runtime_namespace"]
            corrupted = run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "exec",
                    "deployment/workflow-worker",
                    "--",
                    "sh",
                    "-ec",
                    'found=0; for f in "$AGENTX_PLUGIN_CACHE_DIR"/*.mjs; do [ -f "$f" ] || continue; printf corrupt > "$f"; found=1; done; test "$found" = 1',
                ),
                timeout=30,
            )
            assert corrupted.returncode == 0
            refetched = _wait_execution(client, token, _start_fault(client, token, "background", installed_agentx))
            assert refetched["status"] == "succeeded", refetched
            crashed = _wait_execution(client, token, _start_fault(client, token, "crash", installed_agentx))
            assert crashed["status"] == "failed"
            assert crashed["errorCode"] == "PLUGIN_PROTOCOL_ERROR"
            hung = _wait_execution(client, token, _start_fault(client, token, "hang", installed_agentx))
            assert hung["status"] == "failed"
            assert hung["errorCode"] in {"PLUGIN_TIMED_OUT", "NODE_EXECUTION_TIMED_OUT"}
            sync_hung = _wait_execution(client, token, _start_fault(client, token, "sync_hang", installed_agentx))
            assert sync_hung["status"] == "failed"
            assert sync_hung["errorCode"] in {"PLUGIN_TIMED_OUT", "NODE_EXECUTION_TIMED_OUT"}
            orphan = _wait_execution(client, token, _start_fault(client, token, "orphan", installed_agentx))
            assert orphan["status"] == "failed"
            recovered_id = _start_fault(client, token, "recover", installed_agentx)
            running_deadline = time.monotonic() + 20
            while time.monotonic() < running_deadline:
                running_response = client.get(f"/api/v1/executions/{recovered_id}", headers=headers)
                if running_response.status_code in {403, 404}:
                    time.sleep(0.25)
                    continue
                running_response.raise_for_status()
                running = running_response.json()
                if running["status"] == "running":
                    break
                time.sleep(0.25)
            else:
                raise AssertionError("recoverable plugin execution was never running")
            namespace = installed_agentx["runtime_namespace"]
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "rollout",
                    "restart",
                    "deployment/workflow-worker",
                ),
                timeout=60,
            )
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "rollout",
                    "status",
                    "deployment/workflow-worker",
                    "--timeout=300s",
                ),
                timeout=330,
            )
            recovered = _wait_execution(client, token, recovered_id, timeout=120)
            assert recovered["status"] == "succeeded"
    namespace = installed_agentx["runtime_namespace"]
    residue = run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/workflow-worker",
            "--",
            "sh",
            "-ec",
            'found=0; for p in /proc/[0-9]*; do comm=$(cat "$p/comm" 2>/dev/null || true); case "$comm" in agentx-plugin-o*) found=$((found+1));; esac; done; echo $found',
        ),
        timeout=30,
    )
    assert residue.stdout.strip() == "0"


@pytest.mark.cluster
@pytest.mark.runtime
def test_plugin_runner_clears_background_tasks_and_contains_process_crashes(
    installed_agentx: dict[str, str],
) -> None:
    namespace = installed_agentx["runtime_namespace"]
    initialize = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": "init",
            "method": "runner.initialize",
            "params": {"protocolVersion": 2, "sdkApiVersion": 2},
        },
        separators=(",", ":"),
    )
    background = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": "background",
            "method": "node.execute",
            "params": {
                "invocationId": "background",
                "runtimeSource": "export async function execute(){setInterval(()=>{},10000);return {status:'completed',outputs:{main:[]}}}",
                "execution": {"deadline": "2099-01-01T00:00:00Z"},
            },
        },
        separators=(",", ":"),
    )
    completed = run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/workflow-worker",
            "--",
            "sh",
            "-ec",
            'printf "%s\\n%s\\n" "$1" "$2" | timeout 5s node /opt/agentx/plugin-runner/runner.mjs',
            "agentx-plugin-background",
            initialize,
            background,
        ),
        timeout=30,
    )
    messages = [json.loads(line) for line in completed.stdout.splitlines() if line.startswith("{")]
    assert any(message.get("id") == "background" and "result" in message for message in messages)

    crash = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": "crash",
            "method": "node.execute",
            "params": {
                "invocationId": "crash",
                "runtimeSource": "export async function execute(){process.exit(17)}",
                "execution": {"deadline": "2099-01-01T00:00:00Z"},
            },
        },
        separators=(",", ":"),
    )
    crashed = run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/workflow-worker",
            "--",
            "sh",
            "-ec",
            'printf "%s\\n%s\\n" "$1" "$2" | node /opt/agentx/plugin-runner/runner.mjs',
            "agentx-plugin-crash",
            initialize,
            crash,
        ),
        check=False,
        timeout=30,
    )
    assert crashed.returncode != 0
    pods = run(
        (
            "kubectl",
            "-n",
            namespace,
            "get",
            "deployment/workflow-worker",
            "-o",
            "jsonpath={.status.readyReplicas}",
        ),
        timeout=30,
    )
    assert pods.stdout.strip() == "1"


@pytest.mark.cluster
@pytest.mark.runtime
def test_plugin_worker_pool_stays_within_process_and_memory_budgets(
    installed_agentx: dict[str, str],
) -> None:
    namespace = installed_agentx["runtime_namespace"]
    result = run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/workflow-worker",
            "--",
            "sh",
            "-ec",
            "count=0; max=0; for p in /proc/[0-9]*; do "
            'comm=$(cat "$p/comm" 2>/dev/null || true); [ "$comm" = node ] || continue; '
            "cmd=$(tr '\\000' ' ' < \"$p/cmdline\" 2>/dev/null || true); "
            'case "$cmd" in *plugin-runner/runner.mjs*) count=$((count+1)); rss=$(awk \'/VmRSS/{print $2}\' "$p/status"); [ "${rss:-0}" -gt "$max" ] && max=${rss:-0};; esac; '
            'done; printf \'%s %s\\n\' "$count" "$max"',
        ),
        timeout=30,
    )
    count, maximum_rss_kib = (int(value) for value in result.stdout.strip().split())
    assert 0 <= count <= 8
    if count:
        assert maximum_rss_kib <= 192 * 1024


@pytest.mark.cluster
@pytest.mark.runtime
def test_failed_plugin_import_expires_through_the_retention_role(
    installed_agentx: dict[str, str], service_urls: dict[str, str]
) -> None:
    with httpx.Client(base_url=service_urls["web"], timeout=30) as client:
        bootstrap = client.get("/api/v1/bootstrap/status")
        bootstrap.raise_for_status()
        login = client.post(
            "/api/v1/bootstrap" if bootstrap.json()["required"] else "/api/v1/auth/login",
            json={
                "companyName": "Agentx E2E",
                "adminUsername": "admin",
                "adminDisplayName": "Agentx E2E Admin",
                "password": "agentx-e2e-admin-password",
                "locale": "zh-CN",
                "timezone": "Asia/Shanghai",
            }
            if bootstrap.json()["required"]
            else {"username": "admin", "password": "agentx-e2e-admin-password"},
        )
        login.raise_for_status()
        token = login.json()["accessToken"]
        imported = client.post(
            "/api/v1/canvas-plugin-imports",
            headers={"Authorization": f"Bearer {token}"},
            files={"file": ("expired.agentx-plugin", b"invalid", "application/zip")},
        )
        imported.raise_for_status()
        import_id = imported.json()["id"]
        assert imported.json()["status"] == "failed"
        namespace = installed_agentx["control_namespace"]
        expiry_query = (
            "UPDATE canvas_plugin_imports SET expires_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) "  # noqa: S608 -- server-issued UUID.
            f"WHERE id=UUID_TO_BIN('{import_id}');"
        )
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "exec",
                "statefulset/control-mysql",
                "--",
                "sh",
                "-ec",
                f'{MYSQL_ROOT_CLIENT} --batch --skip-column-names -uroot agentx_control -e "$1"',
                "agentx-plugin-expiry",
                expiry_query,
            ),
            timeout=60,
        )
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            response = client.get(
                f"/api/v1/canvas-plugin-imports/{import_id}",
                headers={"Authorization": f"Bearer {token}"},
            )
            if response.status_code == 404:
                return
            time.sleep(1)
        raise AssertionError("expired Canvas Plugin import was not removed")
