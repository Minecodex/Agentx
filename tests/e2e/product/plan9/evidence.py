"""Summarize actual results without treating missing, synthetic or repaired pages as first passes."""

import json
from collections import Counter
from datetime import UTC, datetime
from urllib.parse import urlsplit

from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import redact


def read(directory, name, default=None):
    path = directory / name
    return json.loads(path.read_text()) if path.exists() else default


def select_paid_bindings(rows, namespace):
    selected = []
    for row in rows:
        address = urlsplit(row.get("endpoint") or "")
        if (
            row.get("model") == "glm-5.3-flash"
            and address.scheme == "https"
            and address.hostname == f"plan9-model-relay.{namespace}.svc"
            and address.port == 8443
        ):
            assert address.username is None and address.password is None
            selected.append(
                {
                    "resourceId": row["resourceId"],
                    "model": row["model"],
                    "source": row["source"],
                    "endpointHost": address.hostname,
                    "endpointPort": address.port,
                }
            )
    return selected


def capture_paid_model_scope(suite):
    rows = []
    for query in (
        "SELECT HEX(CAST(JSON_OBJECT('resourceId',LOWER(HEX(resource_id)),'model',JSON_UNQUOTE(JSON_EXTRACT(configuration_json,'$.model')),'endpoint',JSON_UNQUOTE(JSON_EXTRACT(configuration_json,'$.endpoint')),'source','business_binding') AS BINARY)) FROM runtime_resource_bindings WHERE resource_kind='model'",
        "SELECT DISTINCT HEX(CAST(JSON_OBJECT('resourceId',LOWER(HEX(resource_id)),'model',JSON_UNQUOTE(JSON_EXTRACT(design_model_binding_json,'$.configuration.model')),'endpoint',JSON_UNQUOTE(JSON_EXTRACT(design_model_binding_json,'$.configuration.endpoint')),'source','design_binding') AS BINARY)) FROM runtime_calls WHERE call_kind IN ('model','compaction') AND design_model_binding_json IS NOT NULL",
    ):
        rows.extend(
            json.loads(bytes.fromhex(row)) for row in _runtime_mysql(suite["installed"], query).splitlines() if row
        )
    bindings = select_paid_bindings(rows, suite["installed"]["dependencies_namespace"])
    assert bindings, "No frozen model binding matches this run's controlled paid relay"
    scope = {
        "runId": suite["runId"],
        "capturedAt": datetime.now(UTC).isoformat(),
        "readOnly": True,
        "model": "glm-5.3-flash",
        "resourceIds": sorted({row["resourceId"] for row in bindings}),
        "bindings": bindings,
        "scope": "Frozen bindings to this run's TLS ingress for the metered provider. Excludes deterministic fixtures; does not assert acceptance success.",
    }
    (suite["directory"] / "paid-model-resource-scope.json").write_text(json.dumps(scope, indent=2))
    return set(scope["resourceIds"])


def page_metrics(rows):
    passed = [row for row in rows if row["passed"]]
    first = [row for row in passed if len(row["attempts"]) == 1]
    return {
        "attempted": len(rows),
        "passed": len(passed),
        "firstPassCount": len(first),
        "firstPassRate": len(first) / len(rows) if rows else None,
        "finalPassRate": len(passed) / len(rows) if rows else None,
        "businessExecutions": sum(len(row["executions"]) for row in passed),
        "authoringAttempts": sum(len(row["attempts"]) for row in rows),
        "totalSeconds": sum(row.get("elapsedMs", 0) for row in rows) / 1000,
    }


def first_preview_checks(rows):
    checks, missing = [], []
    for page in rows:
        first = page["attempts"][0] if page["attempts"] else None
        report = None
        if first:
            for entry in first.get("entries", []):
                if entry["runId"] != first["run"]["id"] or entry["payload"]["type"] != "page_acknowledged":
                    continue
                candidate = entry["payload"]["result"].get("report")
                if candidate is not None and candidate.get("mode") == "preview":
                    report = candidate
                    break
        if report is None:
            missing.append({"scene": page["scene"], "round": page["round"]})
        else:
            checks.append(
                {
                    "scene": page["scene"],
                    "round": page["round"],
                    "htmlHash": report["htmlHash"],
                    "passed": not report["errors"],
                    "errors": report["errors"],
                }
            )
    return {
        "checked": len(checks),
        "passed": sum(row["passed"] for row in checks),
        "successRateAmongChecked": sum(row["passed"] for row in checks) / len(checks) if checks else None,
        "missing": missing,
        "results": checks,
        "note": "The Agent's first synthetic preview is separate from a complete authoring Run, which can include internal self-corrections. It is not a formal business execution.",
    }


def summarize(suite, budget_before, budget_after):
    directory = suite["directory"]
    pages = (
        read(directory, "page-results.json", [])
        + read(directory, "creation-chain-page-results.json", [])
        + read(directory, "host-recovery-page-results.json", [])
        + read(directory, "receipt-recovery-page-results.json", [])
    )
    first = [page for page in pages if page["passed"] and len(page["attempts"]) == 1]
    passed = [page for page in pages if page["passed"]]
    unique_drafts = {page["draftId"] for page in passed}
    unique_sessions = {page["sessionId"] for page in passed}
    usage, errors, categories = {}, Counter(), Counter()
    for page in pages:
        for entry in page.get("modification", {}).get("entries", []):
            payload = entry["payload"]
            if payload.get("type") == "usage":
                usage[payload["effectId"]] = payload
        for attempt in page["attempts"]:
            if attempt.get("error"):
                errors[attempt["error"].splitlines()[0][:300]] += 1
                categories[attempt.get("failureClass", "unclassified")] += 1
            for entry in attempt.get("entries", []):
                payload = entry["payload"]
                if payload.get("type") == "usage":
                    usage[payload["effectId"]] = payload
    protocol = read(directory, "mcp-protocol-results.json", [])
    native = read(directory, "opencode-results.json", [])
    chains = read(directory, "workflow-ai-results.json", [])
    guidance = {}
    planned_guidance_ids = {value["messageId"] for page in pages for value in page.get("guidanceInputs", [])} | {
        attempt["guidance"]["messageId"] for chain in chains for attempt in chain["attempts"] if attempt.get("guidance")
    }
    for source, rows in (("application_ui_builder", pages), ("workflow_builder", chains)):
        for row in rows:
            for attempt in row["attempts"]:
                for entry in attempt.get("entries", []):
                    payload = entry["payload"]
                    if payload.get("type") != "user" or not payload.get("inputKind"):
                        continue
                    key = f"{entry['runId']}:{payload['messageId']}"
                    guidance[key] = {
                        "source": source,
                        "scene": row["scene"],
                        "runId": entry["runId"],
                        "messageId": payload["messageId"],
                        "kind": payload["inputKind"],
                        "text": payload["text"],
                        "planned": payload["messageId"] in planned_guidance_ids,
                    }
    expected = {(f"S{scene:02}", round_) for scene in range(1, 21) for round_ in range(1, 4)}
    actual = {(page["scene"], page["round"]) for page in passed}
    expected_chains = {"S08-chain", "S12-chain", "S14-chain", "S19-chain"}
    chain_pages = {page["scene"] for page in passed if page["scene"].endswith("-chain")}
    executions = sum(len(page["executions"]) for page in passed)
    attempt_limits = [
        {
            "scene": page["scene"],
            "round": page["round"],
            "runStarts": len(page["attempts"]),
            "authoringAttempts": len(page["attempts"]),
        }
        for page in pages
    ]
    report = {
        "status": "in_progress",
        "model": "glm-5.3-flash",
        "pageInstancesAttempted": len(pages),
        "pageInstancesPassed": len(passed),
        "uniquePageDrafts": len(unique_drafts),
        "uniquePageAuthoringSessions": len(unique_sessions),
        "firstPassCount": len(first),
        "firstPassRate": len(first) / len(pages) if pages else None,
        "finalPassRate": len(passed) / len(pages) if pages else None,
        "actualBusinessExecutions": executions,
        "missingBasePages": sorted(expected - actual),
        "missingCreationChainPages": sorted(expected_chains - chain_pages),
        "generationMatrixPassed": expected.issubset(actual)
        and expected_chains.issubset(chain_pages)
        and len(passed) == len(pages) == len(unique_drafts) == len(unique_sessions)
        and executions >= 128
        and all(len(page["executions"]) >= 2 for page in passed)
        and all(1 <= row["authoringAttempts"] <= 3 for row in attempt_limits),
        "aiAttemptLimits": attempt_limits,
        "protocolScenesPassed": [row["scene"] for row in protocol if row["passed"]],
        "protocolAttemptMetrics": {
            "firstPassCount": sum(row.get("firstAttemptPassed", row["passed"]) for row in protocol),
            "finalPassCount": sum(row["passed"] for row in protocol),
            "attempts": sum(len(row.get("attempts", [row])) for row in protocol),
            "transportRetests": sum(row.get("transportRetests", 0) for row in protocol),
            "failedAttempts": [
                {
                    "scene": row["scene"],
                    "attempt": attempt.get("attempt", 0),
                    "error": attempt["error"],
                    "failedTaskId": attempt.get("failedTask", {}).get("taskId"),
                    "transportFailure": attempt.get("transportFailure"),
                }
                for row in protocol
                for attempt in row.get("attempts", [row])
                if not attempt["passed"]
            ],
        },
        "nativeAiScenesPassed": [row["scene"] for row in native if row["passed"]],
        "canvasAiChainsPassed": [row["scene"] for row in chains if row["passed"]],
        "canvasAiCreation": {
            "attempted": len(chains),
            "passed": sum(row["passed"] for row in chains),
            "firstPassCount": sum(row["passed"] and len(row["attempts"]) == 1 for row in chains),
            "totalSeconds": sum(attempt["elapsedMs"] for row in chains for attempt in row["attempts"]) / 1000,
        },
        "pageCohorts": {
            "base": page_metrics([row for row in pages if row["scene"] in {f"S{index:02}" for index in range(1, 21)}]),
            "creationChains": page_metrics([row for row in pages if row["scene"].endswith("-chain")]),
            "hostRecovery": page_metrics([row for row in pages if row["scene"] == "S01-recovery"]),
            "receiptRecovery": page_metrics([row for row in pages if row["scene"] == "S01-receipt-recovery"]),
        },
        "agentFirstSyntheticPreview": first_preview_checks(pages),
        "pageResultsByScene": {
            scene: page_metrics([row for row in pages if row["scene"] == scene])
            for scene in sorted({row["scene"] for row in pages})
        },
        "firstPassDefinition": "One authoring and business-validation attempt, allowing the Agent's own checks and corrections inside its Run. Planned S08 mid-Run display requirements and the S01 natural-language modification are reported separately; external diagnostic repairs increase the attempt count. Every page has at most three authoring attempts. All failed candidate cohorts remain separate and are never promoted by restarting their exhausted pages. Extra recovery pages and canvas creation are separated from the required 60+4 pages.",
        "designEntryInputTokens": sum(value["inputTokens"] for value in usage.values()),
        "designEntryOutputTokens": sum(value["outputTokens"] for value in usage.values()),
        "designEntryCostMicros": sum(value["costMicros"] for value in usage.values()),
        "pageGenerationSeconds": (
            sum(attempt.get("elapsedMs", 0) for page in pages for attempt in page["attempts"])
            + sum(page.get("modification", {}).get("elapsedMs", 0) for page in pages)
        )
        / 1000,
        "pageBusinessSeconds": sum(execution.get("elapsedMs", 0) for page in passed for execution in page["executions"])
        / 1000,
        "pageTotalSeconds": sum(page.get("elapsedMs", 0) for page in pages) / 1000,
        "internalToolErrors": (
            sum(attempt.get("toolErrors", 0) for page in pages for attempt in page["attempts"])
            + sum(page.get("modification", {}).get("toolErrors", 0) for page in pages)
        ),
        "naturalLanguageModifications": [
            {"scene": page["scene"], "round": page["round"], **page["modification"]}
            for page in pages
            if page.get("modification")
        ],
        "plannedAuthoringGuidance": {
            "page": [
                {"scene": page["scene"], "round": page["round"], "inputs": page["guidanceInputs"]}
                for page in pages
                if page.get("guidanceInputs")
            ],
            "canvas": [
                {"scene": chain["scene"], "runId": attempt["run"], "input": attempt["guidance"]}
                for chain in chains
                for attempt in chain["attempts"]
                if attempt.get("guidance")
            ],
            "note": "Planned S08 display requirements are added through the real creator UI to the original Run. They are reported separately, carry no held-out business inputs, and never reset the three-Run generation/repair limit.",
        },
        "allAuthoringGuidance": list(guidance.values()),
        "additionalDiagnosticGuidance": [value for value in guidance.values() if not value["planned"]],
        "failureCategories": dict(categories),
        "failures": dict(errors),
        "budgetBefore": budget_before,
        "budgetAfter": budget_after,
        "note": "Only this run's real page instances are counted. Pilots, synthetic isolation pages, chat followups and cancellation requests do not increase the required 128 executions. Known usage is deduplicated by Effect; unknown charges remain reserved in the global meter. Complete acceptance also requires the 38 gates and affected regressions.",
    }
    rows = _runtime_mysql(
        suite["installed"],
        "SELECT HEX(CAST(JSON_OBJECT('id',LOWER(HEX(id)),'source',source_kind,'kind',call_kind,'resourceId',LOWER(HEX(resource_id)),'status',status,'inputTokens',input_tokens,'outputTokens',output_tokens,'costMicros',cost_micros,'currency',cost_currency,'estimated',usage_estimated,'errorCode',error_code,'errorMessage',error_message,'startedAt',CAST(started_at AS CHAR),'endedAt',CAST(ended_at AS CHAR)) AS BINARY)) FROM runtime_calls ORDER BY started_at,id",
    )
    ledger = [json.loads(bytes.fromhex(row)) for row in rows.splitlines() if row]
    for row in ledger:
        if row["errorMessage"] is not None:
            row["errorMessage"] = redact(row["errorMessage"])
    (directory / "runtime-call-ledger.json").write_text(json.dumps(ledger, ensure_ascii=False, indent=2))
    paid_resources = capture_paid_model_scope(suite)
    all_model_calls = [row for row in ledger if row["kind"] in {"model", "compaction"}]
    model_calls = [row for row in all_model_calls if row["resourceId"] in paid_resources]
    report["runtimeModelUsage"] = {
        "calls": len(model_calls),
        "inputTokens": sum(row["inputTokens"] for row in model_calls),
        "outputTokens": sum(row["outputTokens"] for row in model_calls),
        "costMicros": sum(row["costMicros"] for row in model_calls),
        "statuses": dict(Counter(row["status"] for row in model_calls)),
        "excludedFixtureOrUnclassifiedCalls": len(all_model_calls) - len(model_calls),
        "scopeEvidence": "paid-model-resource-scope.json",
    }
    native_sources = [
        *native,
        read(directory, "opencode-oauth-results.json", {}),
        read(directory / "cross-entry", "result.json", {}),
    ]
    report["nativeUsageSteps"] = [
        event["part"] for row in native_sources for event in row.get("events", []) if event.get("type") == "step_finish"
    ]
    steps = {step["id"]: step for step in report["nativeUsageSteps"]}
    report["nativeReportedUsage"] = {
        "steps": len(steps),
        "uncachedInputTokens": sum(step["tokens"]["input"] for step in steps.values()),
        "cacheReadTokens": sum(step["tokens"]["cache"]["read"] for step in steps.values()),
        "cacheWriteTokens": sum(step["tokens"]["cache"]["write"] for step in steps.values()),
        "outputTokens": sum(step["tokens"]["output"] for step in steps.values()),
        "reasoningTokens": sum(step["tokens"]["reasoning"] for step in steps.values()),
        "note": "Native client fields are reported separately. The relay ledger is authoritative for the spending cap; the custom provider's native cost field is not used.",
    }
    (directory / "acceptance-summary.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
