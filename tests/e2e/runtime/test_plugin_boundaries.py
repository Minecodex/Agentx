from __future__ import annotations

import json
import os
import time
import uuid
from itertools import pairwise
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

import httpx
import pytest

from tests.e2e.product.test_playwright import (
    _build_canvas_plugin_v2,
    _run_playwright,
    _verify_plugin_trace_degradation,
)
from tests.e2e.runtime.test_plugins import _login, _wait_admission_delivery, _wait_execution
from tests.e2e.runtime.test_runtime import _runtime_mysql
from tests.e2e.support import run

RUNTIME = """
export function resolveDefinition(configuration, upstream) {
  const label = configuration.label || 'first';
  const follow = label === 'follow';
  const schema = follow ? (upstream.main?.schema || {type:'object'})
    : ['file_source','file_read','slow','fast','trace'].includes(label) ? {type:'object',additionalProperties:true}
    : {type:'object',properties:{[label]:{type:'number'}},required:[label],additionalProperties:false};
  const field = follow ? Object.keys(schema.properties || {})[0] : undefined;
  return {status:'complete',inputPorts:[{name:'main',kind:'main',required:true,variadic:false}],
    outputPorts:[{name:'main',kind:'main',required:false,variadic:false},...(field?[{name:field,kind:'main',required:false,variadic:false}]:[])],
    outputSchema:schema, outputPortSchemas:field?{[field]:{type:'object',properties:{value:{type:'number'}},required:['value']}}:{}};
}
export async function execute(ctx) {
  const label = ctx.parameters.label || 'first';
  const started = Date.now();
  if(label==='slow') await new Promise(resolve=>setTimeout(resolve,3000));
  let value;
  if(label==='file_source') {
    const fs=await import('node:fs/promises');
    const file=await fs.open('large.bin','w');
    try { for(let i=0;i<40;i++) await file.write(new Uint8Array(256*1024).fill(0x6a)); }
    finally { await file.close(); }
    value={file:await ctx.artifacts.put({path:'large.bin',fileName:'large.bin',contentType:'application/octet-stream'})};
  } else if(label==='file_read') {
    const input=ctx.inputs.main[0].json.file;
    const local=await ctx.artifacts.read(input);
    const fs=await import('node:fs');
    const crypto=await import('node:crypto');
    const hash=crypto.createHash('sha256'); let bytes=0;
    for await (const chunk of fs.createReadStream(local.path)) { hash.update(chunk); bytes+=chunk.length; }
    let denied=0;
    try { await ctx.artifacts.read({...input,artifactId:'00000000-0000-0000-0000-000000000001'}); } catch { denied++; }
    try { await ctx.artifacts.put({path:'../escape.bin',fileName:'escape.bin',contentType:'application/octet-stream'}); } catch { denied++; }
    value={bytes,sha256:hash.digest('hex'),denied};
  } else if(label==='trace') {
    let effects=0; for(let i=0;i<100;i++) await ctx.trace.span('item-'+i,()=>effects++);
    value={effects};
  } else if(label==='follow') {
    value=ctx.inputs.main[0].json;
    const field=Object.keys(value)[0];
    return {status:'completed',outputs:{main:[{json:value}],[field]:[{json:{value:value[field]}}]}};
  } else if(['slow','fast'].includes(label)) value={started,finished:Date.now(),label};
  else value={[label]:42};
  return {status:'completed',outputs:{main:[{json:value}]}};
}
"""


@pytest.fixture(scope="module")
def boundary_package(installed_agentx: dict[str, str]) -> Path:
    root = Path(installed_agentx["root"])
    pnpm = ("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")
    run((*pnpm, "--filter", "agentx-canvas-plugin-template", "build"), timeout=120)
    run((*pnpm, "--filter", "agentx-canvas-plugin-template", "pack:plugin"), timeout=120)
    with ZipFile(root / "src/plugins/templates/canvas-plugin/dist/acme-json-mapper.agentx-plugin") as archive:
        entries = {name: archive.read(name) for name in archive.namelist()}
    manifest = json.loads(entries["manifest.json"])
    manifest.update(packageId="acme/boundaries", displayName="Boundary Plugin", traceRenderers=[])
    node = json.loads(entries["nodes/json_mapper.json"])
    node.update(nodeType="acme.boundaries", displayName="Boundary Plugin")
    node["parameterSchema"] = {
        "type": "object",
        "properties": {"label": {"type": "string"}},
        "additionalProperties": False,
    }
    node["outputCardinality"] = {"main": "many"}
    node["providers"] = []
    node["uiSchema"] = {"order": ["label"], "fields": {"label": {"control": "text"}}, "canvas": {"role": "default"}}
    entries["manifest.json"] = json.dumps(manifest).encode()
    entries["nodes/json_mapper.json"] = json.dumps(node).encode()
    entries["runtime/entry.js"] = RUNTIME.encode()
    target = Path(installed_agentx["artifact_dir"]) / "boundaries.agentx-plugin"
    with ZipFile(target, "w", ZIP_DEFLATED) as archive:
        for name, content in sorted(entries.items()):
            info = ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)
    return target


def _install(client: httpx.Client, headers: dict[str, str], package: Path) -> None:
    with package.open("rb") as stream:
        response = client.post(
            "/api/v1/canvas-plugin-imports", headers=headers, files={"file": (package.name, stream, "application/zip")}
        )
    response.raise_for_status()
    preview = response.json()
    assert preview["status"] in {"ready", "installed"}, preview
    installed = client.post(
        f"/api/v1/canvas-plugin-imports/{preview['id']}/install",
        headers=headers,
        json={"bundleDigest": preview["bundleDigest"], "enable": True, "setDefault": True},
    )
    installed.raise_for_status()


def _workflow(client: httpx.Client, headers: dict[str, str], labels: list[str]) -> tuple[str, int]:
    response = client.post(
        "/api/v1/workflows",
        headers=headers,
        json={"name": f"Plugin boundaries {uuid.uuid4().hex[:8]}", "visibility": "company"},
    )
    response.raise_for_status()
    workflow_id = response.json()["id"]
    response = client.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
    response.raise_for_status()
    draft = response.json()
    definition = draft["definition"]
    definition["nodes"] = [
        {
            "id": f"node{index}",
            "key": f"node{index}",
            "type": "acme.boundaries",
            "typeVersion": 1,
            "name": label,
            "disabled": False,
            "protected": False,
            "parameters": {"label": label},
            "contextWrites": [],
            "resourceReferences": [],
            "settings": {"timeoutMs": 15000},
        }
        for index, label in enumerate(labels)
    ] + definition["nodes"]
    order = ["__start__", *(f"node{index}" for index in range(len(labels))), "exit"]
    definition["connections"] = [
        {
            "id": f"edge{index}",
            "sourceNodeId": source,
            "sourceHandle": "main",
            "targetNodeId": target,
            "targetHandle": "main",
            "order": 0,
        }
        for index, (source, target) in enumerate(pairwise(order))
    ]
    editor = draft["editorDocument"]
    editor["nodeLayouts"] = [
        {"nodeId": f"node{index}", "x": 300 + index * 280, "y": 200} for index in range(len(labels))
    ] + editor["nodeLayouts"]
    editor["edges"] = [{"edgeId": edge["id"]} for edge in definition["connections"]]
    response = client.put(
        f"/api/v1/workflows/{workflow_id}/draft",
        headers=headers,
        json={"expectedRevision": draft["revision"], "definition": definition, "editorDocument": editor},
    )
    response.raise_for_status()
    return workflow_id, response.json()["revision"]


def _start(client: httpx.Client, headers: dict[str, str], workflow: tuple[str, int]) -> str:
    response = client.post(
        f"/api/v1/workflows/{workflow[0]}/debug-executions",
        headers=headers,
        json={
            "expectedRevision": workflow[1],
            "idempotencyKey": str(uuid.uuid4()),
            "input": {},
            "context": {},
            "mode": "full",
            "targetNodeId": None,
            "inputSource": {},
            "overlayIds": [],
            "sideEffectDecisions": {},
        },
    )
    response.raise_for_status()
    return response.json()["executionId"]


def _outputs(client: httpx.Client, token: str, execution_id: str) -> dict[str, dict[str, object]]:
    execution = _wait_execution(client, token, execution_id)
    assert execution["status"] == "succeeded", execution
    response = client.get(f"/api/v1/executions/{execution_id}/nodes", headers={"Authorization": f"Bearer {token}"})
    response.raise_for_status()
    return {
        node["nodeId"]: node["output"]["main"][0]["json"]
        for node in response.json()["items"]
        if node["nodeType"] == "acme.boundaries"
    }


@pytest.mark.cluster
@pytest.mark.product
def test_dynamic_plugin_chain_in_browser(
    installed_agentx: dict[str, str], service_urls: dict[str, str], boundary_package: Path
) -> None:
    environment = {
        **os.environ,
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_PLUGIN_BOUNDARY_PACKAGE": str(boundary_package),
    }
    _run_playwright(
        "plugin-boundaries", ("tests/canvas-plugin-boundaries.spec.ts",), environment, installed_agentx["root"]
    )


@pytest.mark.cluster
@pytest.mark.runtime
def test_plugin_files_concurrency_trace_and_cleanup(
    installed_agentx: dict[str, str], service_urls: dict[str, str], boundary_package: Path
) -> None:
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        token = _login(client)
        headers = {"Authorization": f"Bearer {token}"}
        _install(client, headers, boundary_package)
        files = _workflow(client, headers, ["file_source", "file_read"])
        slow = _workflow(client, headers, ["slow"])
        fast = _workflow(client, headers, ["fast"])
        trace = _workflow(client, headers, ["trace"])
        _wait_admission_delivery(installed_agentx)
        file_execution = _start(client, headers, files)
        values = _outputs(client, token, file_execution)
        assert values["node1"]["bytes"] == 10 * 1024 * 1024
        assert values["node1"]["sha256"] == values["node0"]["file"]["sha256"]
        assert values["node1"]["denied"] == 2
        slow_id = _start(client, headers, slow)
        fast_id = _start(client, headers, fast)
        fast_value = _outputs(client, token, fast_id)["node0"]
        slow_value = _outputs(client, token, slow_id)["node0"]
        assert fast_value["finished"] < slow_value["finished"], (slow_value, fast_value)
        trace_id = _start(client, headers, trace)
        assert _outputs(client, token, trace_id)["node0"]["effects"] == 100
        deadline = time.monotonic() + 15
        while True:
            count = _runtime_mysql(
                installed_agentx,
                f"SELECT COUNT(*) FROM execution_events WHERE execution_id=UUID_TO_BIN('{uuid.UUID(trace_id)}') AND event_type='plugin.trace.incomplete';",  # noqa: S608 -- UUID parsed before interpolation.
            )
            if int(count) > 0:
                break
            assert time.monotonic() < deadline, "Missing Trace degradation marker"
            time.sleep(0.2)
        (Path(installed_agentx["artifact_dir"]) / "plugin-boundaries.json").write_text(
            json.dumps(
                {
                    "files": values,
                    "slow": slow_value,
                    "fast": fast_value,
                    "traceExecution": trace_id,
                },
                indent=2,
            ),
            encoding="utf-8",
        )
    for deployment in ("workflow-worker", "runtime-gateway"):
        result = run(
            (
                "kubectl",
                "-n",
                installed_agentx["runtime_namespace"],
                "exec",
                f"deployment/{deployment}",
                "--",
                "sh",
                "-ec",
                'find "$AGENTX_PLUGIN_WORK_DIR" -mindepth 1 -maxdepth 1 -type d | wc -l',
            ),
            timeout=30,
        )
        assert result.stdout.strip() == "0", (deployment, result.stdout)


@pytest.mark.cluster
@pytest.mark.product
@pytest.mark.usefixtures("boundary_package")
def test_plugin_publish_versions_and_historical_trace(
    installed_agentx: dict[str, str], service_urls: dict[str, str]
) -> None:
    root = installed_agentx["root"]
    _build_canvas_plugin_v2(root)
    evidence = Path(installed_agentx["artifact_dir"]) / "plugin-baseline-execution.json"
    environment = {
        **os.environ,
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_PLUGIN_EXECUTION_OUTPUT": str(evidence),
    }
    pnpm = ("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")
    _run_playwright("canvas-plugins", ("tests/canvas-plugins.spec.ts",), environment, root)
    _verify_plugin_trace_degradation(pnpm, environment, root, installed_agentx["runtime_namespace"], evidence)
