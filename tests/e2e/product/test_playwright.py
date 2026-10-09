from __future__ import annotations

import json
import os
import subprocess
import tempfile
import uuid
from collections.abc import Sequence
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

import pytest

from tests.e2e.support import MYSQL_ROOT_CLIENT, run, run_playwright


def _build_canvas_plugin_v2(root: str) -> None:
    plugin_root = Path(root) / "src" / "plugins" / "templates" / "canvas-plugin" / "dist"
    source = plugin_root / "acme-json-mapper.agentx-plugin"
    target = plugin_root / "acme-json-mapper-v2.agentx-plugin"
    with ZipFile(source) as archive:
        entries = {name: archive.read(name) for name in archive.namelist()}
    manifest = json.loads(entries["manifest.json"])
    manifest["packageVersion"] = "2.0.0"
    node = json.loads(entries["nodes/json_mapper.json"])
    node["version"] = 2
    entries["manifest.json"] = json.dumps(manifest, ensure_ascii=False, separators=(",", ":")).encode()
    entries["nodes/json_mapper.json"] = json.dumps(node, ensure_ascii=False, separators=(",", ":")).encode()
    runtime = entries["runtime/entry.js"].decode()
    entries["runtime/entry.js"] = runtime.replace(
        "label: context.parameters.label",
        "label: context.parameters.label, pluginVersion: '2.0.0'",
    ).encode()
    with ZipFile(target, "w", ZIP_DEFLATED) as archive:
        for name, content in sorted(entries.items()):
            info = ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)

    race_target = plugin_root / "acme-race.agentx-plugin"
    race_entries = dict(entries)
    race_manifest = json.loads(race_entries["manifest.json"])
    race_manifest.update(packageId="acme/race", packageVersion="1.0.0", displayName="Race Plugin")
    race_node = json.loads(race_entries["nodes/json_mapper.json"])
    race_node.update(nodeType="acme.race", version=1, displayName="Race Plugin")
    race_entries["manifest.json"] = json.dumps(race_manifest, ensure_ascii=False, separators=(",", ":")).encode()
    race_entries["nodes/json_mapper.json"] = json.dumps(race_node, ensure_ascii=False, separators=(",", ":")).encode()
    with ZipFile(race_target, "w", ZIP_DEFLATED) as archive:
        for name, content in sorted(race_entries.items()):
            info = ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)


def _run_playwright(
    suite: str,
    tests: Sequence[str],
    environment: dict[str, str],
    root: str,
) -> None:
    run_playwright(Path(root), suite, tests, environment)


def _assert_execution_context_snapshot(installed_agentx: dict[str, str], evidence_path: Path) -> None:
    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    execution_id = str(uuid.UUID(evidence["executionId"]))
    fields = (
        "'executionId',JSON_UNQUOTE(JSON_EXTRACT(execution_context_json,'$.id'))",
        "'workflowName',JSON_UNQUOTE(JSON_EXTRACT(execution_context_json,'$.workflow.name'))",
        "'departmentName',JSON_UNQUOTE(JSON_EXTRACT(execution_context_json,'$.initiator.department.name'))",
        "'roleCodes',JSON_EXTRACT(execution_context_json,'$.initiator.roles.codes')",
        "'initiatorType',JSON_UNQUOTE(JSON_EXTRACT(execution_context_json,'$.initiator.type'))",
        "'contextWorkflowName',JSON_UNQUOTE(JSON_EXTRACT(e.output_json,'$.context_workflow_name'))",
    )
    query = (
        f"SELECT JSON_OBJECT({','.join(fields)}) FROM execution_snapshots s "  # noqa: S608 -- UUID is normalized.
        "JOIN workflow_executions e ON e.tenant_id=s.tenant_id AND e.id=s.execution_id "
        f"WHERE s.execution_id=UUID_TO_BIN('{execution_id}')"
    )
    result = run(
        (
            "kubectl",
            "-n",
            installed_agentx["runtime_namespace"],
            "exec",
            "statefulset/runtime-mysql",
            "--",
            "sh",
            "-ec",
            f'{MYSQL_ROOT_CLIENT} '
            '--batch --skip-column-names -uroot agentx_runtime -e "$1"',
            "agentx-e2e-query",
            query,
        ),
        timeout=60,
    )
    snapshot = json.loads(result.stdout.strip())
    assert snapshot == {
        "executionId": execution_id,
        "workflowName": evidence["workflowName"],
        "departmentName": evidence["departmentName"],
        "roleCodes": evidence["roleCodes"],
        "initiatorType": "user",
        "contextWorkflowName": evidence["contextWorkflowName"],
    }


def _verify_plugin_trace_degradation(
    environment: dict[str, str], root: str, namespace: str, evidence_path: Path
) -> None:
    execution_id = json.loads(evidence_path.read_text(encoding="utf-8"))["executionId"]
    trace_environment = {**environment, "AGENTX_E2E_DEGRADED_EXECUTION_ID": execution_id}
    run(("kubectl", "-n", namespace, "scale", "statefulset/clickhouse", "--replicas=0"), timeout=60)
    try:
        run(("kubectl", "-n", namespace, "rollout", "status", "statefulset/clickhouse", "--timeout=180s"), timeout=200)
        _run_playwright(
            "plugin-trace-degraded",
            ("tests/trace-degraded.spec.ts",),
            {**trace_environment, "AGENTX_E2E_TRACE_PHASE": "degraded"},
            root,
        )
    finally:
        run(("kubectl", "-n", namespace, "scale", "statefulset/clickhouse", "--replicas=1"), timeout=60)
        run(("kubectl", "-n", namespace, "rollout", "status", "statefulset/clickhouse", "--timeout=300s"), timeout=330)
    _run_playwright(
        "plugin-trace-recovered",
        ("tests/trace-degraded.spec.ts",),
        {**trace_environment, "AGENTX_E2E_TRACE_PHASE": "recovered"},
        root,
    )


@pytest.mark.cluster
@pytest.mark.product
def test_product_playwright_suite(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    e2e_providers: dict[str, str],
    code_egress_fixtures: dict[str, str],
) -> None:
    environment = os.environ.copy()
    environment["AGENTX_E2E_RUN_ID"] = installed_agentx["run_id"]
    environment["AGENTX_E2E_STAGE"] = "helm-agentxctl"
    environment["AGENTX_E2E_BASE_URL"] = service_urls["web"]
    environment["AGENTX_E2E_RUNTIME_URL"] = service_urls["runtime"]
    environment["AGENTX_E2E_ECHO_BASE_URL"] = e2e_providers["echo_mcp"]
    environment["AGENTX_E2E_LIGHTRAG_BASE_URL"] = e2e_providers["lightrag"]
    environment["AGENTX_E2E_MEM0_BASE_URL"] = e2e_providers["mem0"]
    environment["AGENTX_E2E_CODE_EGRESS_HOST"] = code_egress_fixtures["host"]
    environment["AGENTX_E2E_CODE_HTTP_PORT"] = code_egress_fixtures["http_port"]
    environment["AGENTX_E2E_CODE_TCP_PORT"] = code_egress_fixtures["tcp_port"]
    pnpm = ("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")
    subprocess.run(
        [*pnpm, "--filter", "agentx-canvas-plugin-template", "build"],
        check=True,
        shell=False,
        cwd=installed_agentx["root"],
    )
    subprocess.run(
        [*pnpm, "--filter", "agentx-canvas-plugin-template", "pack:plugin"],
        check=True,
        shell=False,
        cwd=installed_agentx["root"],
    )
    _build_canvas_plugin_v2(installed_agentx["root"])
    with tempfile.TemporaryDirectory(prefix="agentx-e2e-context-") as context_dir:
        environment["AGENTX_V2_08_CONTEXT_OUTPUT"] = str(Path(context_dir) / "v2-08-context.json")
        execution_context_evidence = Path(context_dir) / "execution-context.json"
        plugin_execution_evidence = Path(context_dir) / "plugin-execution.json"
        environment["AGENTX_EXECUTION_CONTEXT_EVIDENCE_OUTPUT"] = str(execution_context_evidence)
        environment["AGENTX_PLUGIN_EXECUTION_OUTPUT"] = str(plugin_execution_evidence)
        root = installed_agentx["root"]
        suites = (
            ("canvas-plugins", ("tests/canvas-plugins.spec.ts",)),
            ("api-first", ("tests/v2-08-api-first.spec.ts",)),
            ("application-docs", ("tests/application-integration-docs.spec.ts",)),
            ("application-channels", ("tests/application-channel-forms.spec.ts",)),
            ("knowledge-management", ("tests/knowledge-management.spec.ts",)),
            ("execution-filters", ("tests/execution-filters.spec.ts",)),
            ("workflow4", ("tests/workflow4-closure.spec.ts", "--retries=1")),
            (
                "control-ui",
                (
                    "tests/m2.1-control-plane.spec.ts",
                    "tests/resource-grant-requests.spec.ts",
                ),
            ),
            (
                "product-closure",
                (
                    "tests/m6-workflow-studio.spec.ts",
                    "tests/workflow-loop-input-regressions.spec.ts",
                    "tests/m6-local-builtins.spec.ts",
                    "tests/m7-business-closure.spec.ts",
                    "tests/safe-deletion.spec.ts",
                ),
            ),
            ("workflow-multi-exit", ("tests/workflow-multi-exit.spec.ts",)),
            ("workflow-performance", ("tests/workflow-canvas-performance.spec.ts",)),
        )
        only_suites = {
            suite.strip() for suite in os.environ.get("AGENTX_E2E_ONLY_SUITE", "").split(",") if suite.strip()
        }
        playwright_grep = os.environ.get("AGENTX_E2E_PLAYWRIGHT_GREP")
        failures: list[str] = []
        for suite, tests in suites:
            if only_suites and suite not in only_suites:
                continue
            selected_tests = (*tests, "--grep", playwright_grep) if playwright_grep else tests
            try:
                _run_playwright(suite, selected_tests, environment, root)
                if suite == "canvas-plugins" and not playwright_grep:
                    _verify_plugin_trace_degradation(
                        environment,
                        root,
                        installed_agentx["runtime_namespace"],
                        plugin_execution_evidence,
                    )
            except subprocess.CalledProcessError:
                failures.append(suite)
                continue
            if suite == "product-closure" and not playwright_grep:
                _assert_execution_context_snapshot(installed_agentx, execution_context_evidence)
        if not only_suites or "session-diagnostics" in only_suites:
            try:
                _run_playwright(
                    "session-diagnostics",
                    ("tests/agent-sessions-diagnostics.spec.ts",),
                    environment,
                    root,
                )
            except subprocess.CalledProcessError:
                failures.append("session-diagnostics")
        assert not failures, f"Playwright suites failed: {', '.join(failures)}"
