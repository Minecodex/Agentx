"""Prove creator guidance reached the same durable Core, without another Run."""

import json
import uuid

from tests.e2e.runtime.test_agent_attachments import _control_mysql


def verify_guidance(suite, attempts, *, page=False):
    proof = []
    sessions = {attempt["sessionId"] for attempt in attempts if attempt.get("guidance")}
    for session in sessions:
        identifier = uuid.UUID(session).hex
        expected = {
            uuid.UUID(attempt["run"]["id"] if page else attempt["run"]).hex
            for attempt in attempts
            if attempt["sessionId"] == session
        }
        actual = set(
            _control_mysql(
                suite["installed"],
                f"SELECT LOWER(HEX(id)) FROM design_agent_runs WHERE session_id=UNHEX('{identifier}')",  # noqa: S608 -- validated UUID.
            ).splitlines()
        )
        assert actual == expected, "Guidance must not create an unreported additional Run"
    for attempt in attempts:
        guidance = attempt.get("guidance")
        if not guidance:
            continue
        session = uuid.UUID(attempt["sessionId"]).hex
        run = uuid.UUID(attempt["run"]["id"] if page else attempt["run"])
        assert uuid.UUID(guidance["runId"]) == run
        message = uuid.UUID(guidance["messageId"])
        core_id = f"design-input:{run}:{message}"
        raw = _control_mysql(
            suite["installed"],
            f"SELECT HEX(CAST(state_json AS BINARY)) FROM design_agent_sessions WHERE id=UNHEX('{session}')",  # noqa: S608 -- validated UUID in the owned isolated database.
        )
        core = json.loads(bytes.fromhex(raw.strip()))
        messages = [value for value in core["messages"] if value["messageId"] == core_id]
        pending = [
            value
            for name in ("steeringQueue", "followUpQueue")
            for value in core.get(name, [])
            if value["messageId"] == core_id
        ]
        status = _control_mysql(
            suite["installed"],
            f"SELECT status FROM design_agent_runs WHERE id=UNHEX('{run.hex}')",  # noqa: S608 -- validated UUID.
        ).strip()
        assert len(messages) <= 1 and len(pending) <= 1
        completed = attempt["run"].get("status") == "completed" if page else attempt.get("error") is None
        if status == "completed" or completed:
            assert len(messages) == 1 and not pending, "A completed original Run must consume its accepted guidance"
        if messages:
            assert messages[0]["role"] == "user" and messages[0]["content"] == guidance["text"]
        proof.append(
            {
                "sessionId": str(uuid.UUID(hex=session)),
                "runId": str(run),
                "messageId": str(message),
                "runStatus": status,
                "observedCompletedBeforePublication": completed,
                "consumedCount": len(messages),
                "pendingCount": len(pending),
                "createdAnotherRun": False,
            }
        )
    return proof
