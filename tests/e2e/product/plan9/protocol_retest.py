"""Bounded, explicitly new protocol executions after a malformed model stream."""


class McpTaskFailure(AssertionError):
    def __init__(self, task):
        self.task = task
        super().__init__(task)


def is_model_stream_interruption(error):
    if not isinstance(error, McpTaskFailure) or error.task.get("state") != "failed":
        return False
    detail = error.task.get("error") or {}
    if not isinstance(detail, dict) or not isinstance(detail.get("errors", []), list):
        return False
    primary = detail.get("primaryError") or {}
    errors = [primary, *detail.get("errors", [])]
    return detail.get("code") == "AGENT_MODEL_ERROR" and all(
        isinstance(item, dict)
        and item.get("code") == "AGENT_MODEL_ERROR"
        and item.get("message") == "model effect failed: model stream ended before finish_reason and [DONE]"
        for item in errors
    )


def fresh_protocol_case(original, attempt):
    """Fresh synthetic references prevent replaying the failed task's writes."""
    assert attempt in (1, 2), "At most two protocol transport retests"
    replacements = {
        value["request_id"]: f"{value['request_id']}-mcp-retest-{attempt}"
        for value in original["inputs"]
        if "request_id" in value
    }

    def replace(value):
        if isinstance(value, dict):
            return {key: replace(item) for key, item in value.items()}
        if isinstance(value, list):
            return [replace(item) for item in value]
        return replacements.get(value, value) if isinstance(value, str) else value

    return replace(original)
