"""Cross-Agent routing cannot reset authoring budgets or replay an unknown business write."""

import json
from pathlib import Path

import pytest

from tests.e2e.product.plan9 import creation_chain_repair as repair
from tests.e2e.product.plan9 import test_creation_chains as chains
from tests.e2e.product.plan9.creation_chains import bind_wait_input_paths


def test_schema_diagnostic_retains_type_without_revealing_held_out_values():
    primary = {"code": "CODE_OUTPUT_SCHEMA_VALIDATION_FAILED", "message": '1212.78 is not of type "integer"'}
    result = repair.schema_type_diagnostic(primary)
    assert result == {"keyword": "type", "expectedType": "integer"}
    assert "1212" not in json.dumps(result)
    primary["message"] = '"secret@example.test" is not of type "number"'
    assert repair.schema_type_diagnostic(primary) == {"keyword": "type", "expectedType": "number"}
    primary["message"] = 'Held-out secret contains type "integer" somewhere'
    assert repair.schema_type_diagnostic(primary) is None
    primary.update(code="OTHER", message='1212.78 is not of type "integer"')
    assert repair.schema_type_diagnostic(primary) is None


def fake_chain(monkeypatch, tmp_path, *, failed_iterations=1, canvas_attempts=1, page_failure="workflow"):
    calls = []
    monkeypatch.setattr(chains, "browser_environment", lambda _: {})

    def publish(_, generated, **options):
        path = options["directory"] / "scenarios.json"
        path.write_text(
            json.dumps({"budget": options["page_attempt_budget"], "version": generated[0]["publishedVersionId"]})
        )
        return path

    def browser(_, name, spec, environment):
        iteration = int(name.split("-")[-2])
        if name.endswith("canvas"):
            blueprint = json.loads(Path(environment["AGENTX_E2E_PLAN9_WORKFLOW_BLUEPRINTS"]).read_text())[0]
            calls.append(("canvas", blueprint["attemptLimit"]))
            count = canvas_attempts if iteration == 0 else 1
            value = {
                "scene": "S19",
                "workflowId": "workflow",
                "publishedVersionId": f"version-{iteration}",
                "passed": True,
                "attempts": [{"run": f"canvas-{iteration}-{i}"} for i in range(count)],
            }
            Path(environment["AGENTX_E2E_PLAN9_WORKFLOW_REPORT"]).write_text(json.dumps([value]))
        else:
            case = json.loads(Path(environment["AGENTX_E2E_PLAN9_SCENARIOS"]).read_text())
            calls.append(("page", case["budget"]))
            passed = iteration >= failed_iterations
            count = case["budget"] if page_failure == "page" and not passed else 1
            value = {
                "scene": "S19-chain",
                "round": 1,
                "passed": passed,
                "draftId": f"draft-{iteration}",
                "sessionId": f"session-{iteration}",
                "attempts": [{"run": {"id": f"page-{iteration}-{i}"}} for i in range(count)],
                "executions": [{}, {}] if passed else [],
            }
            if not passed and page_failure == "workflow":
                value["workflowFailure"] = {"task": {"state": "failed"}}
            report = Path(environment["AGENTX_E2E_PLAN9_REPORT"])
            report.with_suffix("").mkdir()
            (report.with_suffix("") / "row.json").write_text(json.dumps(value))
        return {"exitCode": 0}

    def repaired(_, blueprint, generated, failure, directory, remaining):
        calls.append(("repair", remaining))
        return {
            **blueprint,
            "versionId": generated["publishedVersionId"],
            "prompt": "Canvas receives runtime diagnostic",
        }

    monkeypatch.setattr(chains, "publish_applications", publish)
    monkeypatch.setattr(chains, "browser", browser)
    monkeypatch.setattr(chains, "repair_blueprint", repaired)
    suite = {"directory": tmp_path}
    blueprint = {"scene": "S19", "workflowId": "workflow", "prompt": "original"}
    return suite, blueprint, calls


def test_workflow_failure_routes_to_canvas_and_carries_both_budgets(monkeypatch, tmp_path):
    suite, blueprint, calls = fake_chain(monkeypatch, tmp_path)
    workflow, page = chains.run_chain(suite, blueprint, lambda *_: None)
    assert workflow["passed"] and page["passed"]
    assert calls == [("canvas", 3), ("page", 3), ("repair", 2), ("canvas", 2), ("page", 2)]
    assert len(workflow["attempts"]) == len(page["attempts"]) == 2
    assert [attempt["workflowVersionId"] for attempt in page["attempts"]] == ["version-0", "version-1"]
    assert page["draftId"] == "draft-1" and "workflowFailure" not in page


def test_static_repairs_consume_canvas_budget_before_runtime_repair(monkeypatch, tmp_path):
    suite, blueprint, calls = fake_chain(monkeypatch, tmp_path, canvas_attempts=2)
    workflow, page = chains.run_chain(suite, blueprint, lambda *_: None)
    assert workflow["passed"] and page["passed"]
    assert calls == [("canvas", 3), ("page", 3), ("repair", 1), ("canvas", 1), ("page", 2)]
    assert len(workflow["attempts"]) == 3


def test_workflow_versions_cannot_create_a_fourth_page_or_canvas_run(monkeypatch, tmp_path):
    suite, blueprint, calls = fake_chain(monkeypatch, tmp_path, failed_iterations=10)
    workflow, page = chains.run_chain(suite, blueprint, lambda *_: None)
    assert not workflow["passed"] and not page["passed"]
    assert [remaining for kind, remaining in calls if kind == "canvas"] == [3, 2, 1]
    assert [remaining for kind, remaining in calls if kind == "page"] == [3, 2, 1]
    assert len(workflow["attempts"]) == len(page["attempts"]) == 3


def test_html_failure_exhausts_its_page_budget_without_changing_workflow(monkeypatch, tmp_path):
    suite, blueprint, calls = fake_chain(monkeypatch, tmp_path, page_failure="page")
    workflow, page = chains.run_chain(suite, blueprint, lambda *_: None)
    assert not workflow["passed"] and not page["passed"]
    assert calls == [("canvas", 3), ("page", 3)]
    assert len(page["attempts"]) == 3


def test_unknown_or_unsettled_business_effect_prevents_new_validation(monkeypatch):
    with pytest.raises(AssertionError, match="cannot be replayed"):
        repair.known_runtime_failure({}, {"state": "outcome_unknown"})
    effect = {"effectId": "original", "settled": True, "unknown": True}
    monkeypatch.setattr(repair, "_runtime_mysql", lambda *_: json.dumps(effect).encode().hex())
    task = {"state": "failed", "taskId": "00000000-0000-0000-0000-000000000001"}
    with pytest.raises(AssertionError):
        repair.known_runtime_failure({"installed": {}}, task)
    effect.update(unknown=False, settled=False)
    with pytest.raises(AssertionError):
        repair.known_runtime_failure({"installed": {}}, task)
    effect["settled"] = True
    assert repair.known_runtime_failure({"installed": {}}, task)["effects"] == [effect]


def test_read_only_python_diagnostic_exposes_swallowed_keyword_mismatch():
    node = {
        "type": "code",
        "parameters": {
            "runner": "python",
            "inputs": {"fields": {"crmText": {"kind": "reference"}}},
            "source": "def main(crm_text=None, **_):\n    return crm_text\n",
        },
    }
    diagnostic = repair.python_binding_diagnostic(node)
    assert diagnostic["unmatchedBoundKeywords"] == ["crmText"]
    assert diagnostic["unboundNamedParameters"] == ["crm_text"]
    assert diagnostic["hasCatchAllKeywords"]


def wait_node(identifier, key="delivery"):
    return {
        "id": identifier,
        "key": key,
        "type": "await_input",
        "parameters": {
            "responseSchema": {
                "type": "object",
                "properties": {"date": {"type": "string"}, "contact": {"type": "string"}},
            }
        },
    }


def test_held_out_inputs_follow_actual_ai_node_identity_without_changing_values():
    case = {"waitInputs": [{"/delivery": [{"date": "unseen-date", "contact": "unseen-contact"}]}]}
    mapped, proof = bind_wait_input_paths(
        case, {"nodes": [wait_node("delivery")]}, {"nodes": [wait_node("await_delivery")]}
    )
    assert mapped == [{"/await_delivery": case["waitInputs"][0]["/delivery"]}]
    assert list(case["waitInputs"][0]) == ["/delivery"]
    assert proof == [{"fixturePrefix": "/delivery", "publishedNodePrefix": "/await_delivery"}]


def test_child_wait_paths_keep_the_real_immutable_child_and_change_only_parent_prefix():
    original = {
        "id": "collect",
        "key": "collect",
        "type": "sub_workflow",
        "parameters": {"workflowVersionId": "child-v1"},
    }
    actual = {**original, "id": "collect_details"}
    case = {"waitInputs": [{"/collect/delivery": [{"date": "unseen"}]}]}
    mapped, _ = bind_wait_input_paths(case, {"nodes": [original]}, {"nodes": [actual]})
    assert mapped == [{"/collect_details/delivery": [{"date": "unseen"}]}]
    actual["parameters"] = {"workflowVersionId": "different-child"}
    with pytest.raises(AssertionError, match="specified immutable child"):
        bind_wait_input_paths(case, {"nodes": [original]}, {"nodes": [actual]})


def test_ambiguous_or_changed_business_waits_cannot_be_guessed():
    source, case = {"nodes": [wait_node("delivery")]}, {"waitInputs": [{"/delivery": [{}]}]}
    with pytest.raises(AssertionError, match="Ambiguous"):
        bind_wait_input_paths(case, source, {"nodes": [wait_node("one", "one"), wait_node("two", "two")]})
    actual = wait_node("await_delivery")
    actual["parameters"]["responseSchema"]["properties"] = {"unrelated": {"type": "string"}}
    with pytest.raises(AssertionError, match="business fields"):
        bind_wait_input_paths(case, source, {"nodes": [actual]})


def test_wait_prefix_matching_does_not_bind_an_unrelated_path():
    with pytest.raises(AssertionError, match="No unique"):
        bind_wait_input_paths(
            {"waitInputs": [{"/delivery_other": [{}]}]},
            {"nodes": [wait_node("delivery")]},
            {"nodes": [wait_node("await_delivery")]},
        )
