"""Transport retests retain real failures and never replay their business references."""

import copy
import json
import uuid
from contextlib import contextmanager

import pytest

from tests.e2e.product.plan9 import test_mcp_protocol as protocol
from tests.e2e.product.plan9.protocol_retest import (
    McpTaskFailure,
    fresh_protocol_case,
    is_model_stream_interruption,
)


def interrupted():
    primary = {
        "code": "AGENT_MODEL_ERROR",
        "message": "model effect failed: model stream ended before finish_reason and [DONE]",
    }
    return McpTaskFailure(
        {"taskId": "original-failed-task", "state": "failed", "error": {**primary, "primaryError": primary}}
    )


def scenario():
    return {
        "scene": "S18",
        "applicationId": "same-published-application",
        "crm": True,
        "inputs": [{"request_id": "original-0", "amount": 120.25}, {"request_id": "original-1", "amount": 340.5}],
        "expected": [{"reference": "original-0"}, {"reference": "original-1"}],
    }


def harness(
    monkeypatch, tmp_path, outcomes, *, writes_after_failure=1, unreadable_writes=False, failed_index=0, case=None
):
    original = case or scenario()
    (tmp_path / "scenario-preparation.json").write_text(json.dumps([{"scene": "S18", "prepared": True}]))
    scopes, writes = [], {}
    keys = []

    class Client:
        def __init__(self):
            self.requests = {}
            self.records = []

        def submit(self, _name, _revision, value, request_id=None):
            identity = request_id or str(uuid.uuid4())
            self.requests.setdefault(identity, {"taskId": str(uuid.uuid4())})
            return self.requests[identity]

        def rpc(self, _method, _parameters):
            return {"isError": False}

        def task(self, _identifier):
            return {"state": "cancelled"}

    @contextmanager
    def client(_suite, case, _label):
        scopes.append(copy.deepcopy(case))
        key = {"id": str(uuid.uuid4())}
        keys.append(key)
        yield Client(), {"toolName": "run_application", "revision": 1}, key

    def settle(_suite, _client, task_id, case, index, **_options):
        failure = outcomes[len(scopes) - 1]
        if index == failed_index and failure:
            if case.get("crm"):
                writes[case["inputs"][index]["request_id"]] = writes_after_failure
            raise failure
        return {"taskId": task_id, "state": "completed"}, []

    def crm(_suite, reference):
        if unreadable_writes and reference in writes:
            raise ConnectionError("The original write count is unavailable")
        return [{}] * writes.get(reference, 0)

    monkeypatch.setattr(protocol, "application_client", client)
    monkeypatch.setattr(protocol, "prepare_files", lambda _client, value: value)
    monkeypatch.setattr(protocol, "settle", settle)
    monkeypatch.setattr(protocol, "crm_records", crm)
    monkeypatch.setattr(protocol, "check_result", lambda *_args: {"callerId": uuid.UUID(keys[-1]["id"]).hex})
    return {"directory": tmp_path, "cases": [original]}, scopes, writes


@pytest.mark.parametrize("failed_index", [0, 1])
@pytest.mark.parametrize("write_count", [0, 1])
def test_confirmed_stream_interruption_uses_new_references_and_retains_original_failure(
    monkeypatch, tmp_path, failed_index, write_count
):
    suite, scopes, writes = harness(
        monkeypatch, tmp_path, [interrupted(), None], failed_index=failed_index, writes_after_failure=write_count
    )
    original = copy.deepcopy(suite["cases"][0])
    result = protocol.verify_applications(suite)[0]
    assert result["passed"] and not result["firstAttemptPassed"]
    assert result["transportRetests"] == 1 and len(result["executions"]) == 2
    assert result["attempts"][0]["failedTask"]["taskId"] == "original-failed-task"
    assert result["attempts"][0]["transportFailure"]["writeCountAtFailure"] == write_count
    assert len(result["attempts"][0]["executions"]) == failed_index
    assert writes[f"original-{failed_index}"] == write_count
    assert scopes[1]["inputs"][0]["request_id"] == "original-0-mcp-retest-1"
    assert scopes[1]["expected"][0]["reference"] == "original-0-mcp-retest-1"
    assert scopes[1]["applicationId"] == original["applicationId"]
    assert suite["cases"][0] == original
    persisted = json.loads((tmp_path / "mcp-protocol-results.json").read_text())[0]
    assert persisted["attempts"][0]["error"] and persisted["error"] is None


def test_three_transport_attempts_exhaust_the_limit_without_a_fourth(monkeypatch, tmp_path):
    suite, scopes, _ = harness(monkeypatch, tmp_path, [interrupted(), interrupted(), interrupted()])
    with pytest.raises(AssertionError):
        protocol.verify_applications(suite)
    assert len(scopes) == 3
    references = [case["inputs"][0]["request_id"] for case in scopes]
    assert references == ["original-0", "original-0-mcp-retest-1", "original-0-mcp-retest-2"]
    result = json.loads((tmp_path / "mcp-protocol-results.json").read_text())[0]
    assert not result["passed"] and len(result["attempts"]) == 3


@pytest.mark.parametrize("unreadable,writes", [(False, 2), (True, 1)])
def test_unknown_or_unexpected_original_writes_block_retest(monkeypatch, tmp_path, unreadable, writes):
    suite, scopes, _ = harness(
        monkeypatch, tmp_path, [interrupted(), None], writes_after_failure=writes, unreadable_writes=unreadable
    )
    with pytest.raises(AssertionError):
        protocol.verify_applications(suite)
    assert len(scopes) == 1


def test_schema_failures_and_unknown_tool_results_are_not_transport_retests(monkeypatch, tmp_path):
    failures = [AssertionError("output schema failed"), interrupted(), interrupted(), interrupted()]
    failures[1].task["state"] = "outcome_unknown"
    failures[2].task["error"]["primaryError"] = {"code": "AGENT_TOOL_OUTCOME_UNKNOWN", "message": "unknown write"}
    failures[3].task["error"]["errors"] = [{"code": "AGENT_TOOL_OUTCOME_UNKNOWN", "message": "parallel unknown write"}]
    failures.append(McpTaskFailure({"state": "failed", "error": "unstructured error"}))
    for index, failure in enumerate(failures):
        assert not is_model_stream_interruption(failure)
        folder = tmp_path / str(index)
        folder.mkdir()
        suite, scopes, _ = harness(monkeypatch, folder, [failure, None])
        with pytest.raises(AssertionError):
            protocol.verify_applications(suite)
        assert len(scopes) == 1


def test_transport_case_rewrite_is_bounded_and_preserves_business_values():
    case = scenario()
    fresh = fresh_protocol_case(case, 2)
    assert fresh["inputs"][1]["amount"] == case["inputs"][1]["amount"]
    assert fresh["expected"][1]["reference"] != case["expected"][1]["reference"]
    with pytest.raises(AssertionError):
        fresh_protocol_case(case, 3)


@pytest.mark.parametrize(
    "inputs",
    [
        [
            {"customer": "Acme", "quantity": 2, "unitPrice": 12.5, "discountPercent": 0},
            {"customer": "Beta", "quantity": 5, "unitPrice": 8.25, "discountPercent": 10},
        ],
        [
            {"department": "Sales", "items": [{"sku": "A", "quantity": 2}]},
            {"department": "Finance", "items": [{"sku": "B", "quantity": 3}]},
        ],
        [
            {"order_id": "A", "priority": "high", "insured": True, "weight_kg": 1.25, "packages": []},
            {"order_id": "B", "priority": "low", "insured": False, "weight_kg": 2.5, "packages": []},
        ],
        [{"message": "first conversation"}, {"message": "second conversation"}],
    ],
)
def test_real_matrix_shapes_without_business_reference_complete_and_retest(monkeypatch, tmp_path, inputs):
    case = {"scene": "no-reference", "inputs": inputs, "expected": [{}, {}]}
    original = copy.deepcopy(case)
    suite, scopes, _ = harness(monkeypatch, tmp_path, [interrupted(), None], case=case)
    result = protocol.verify_applications(suite)[0]
    assert result["passed"] and result["transportRetests"] == 1
    assert scopes == [original, original] and suite["cases"][0] == original
    assert result["attempts"][0]["callerKeyId"] != result["attempts"][1]["callerKeyId"]
    assert all(attempt["businessReferences"] == [] for attempt in result["attempts"])
    assert len(result["executions"]) == 2
