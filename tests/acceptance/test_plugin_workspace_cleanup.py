import pytest

from tests.e2e.runtime import test_plugin_boundaries as boundaries
from tests.e2e.support import Result


@pytest.mark.parametrize("persistent", [False, True])
def test_plugin_workspace_cleanup_waits_for_zero_and_rejects_persistent_residue(monkeypatch, persistent):
    clock = [0.0]
    observed = []
    monkeypatch.setattr(boundaries.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(boundaries.time, "sleep", lambda value: clock.__setitem__(0, clock[0] + value))

    def command(arguments, timeout):
        assert 0 < timeout <= 30
        clock[0] += 1
        deployment = arguments[4]
        count = 1 if deployment.endswith("workflow-worker") and (persistent or len(observed) < 2) else 0
        observed.append(count)
        return Result(tuple(arguments), f"{count}\n", "", 0)

    monkeypatch.setattr(boundaries, "run", command)
    if persistent:
        with pytest.raises(AssertionError, match=r"workflow-worker.*1"):
            boundaries._assert_invocation_workspaces_reclaimed({"runtime_namespace": "owned-test"}, timeout_seconds=5)
    else:
        boundaries._assert_invocation_workspaces_reclaimed({"runtime_namespace": "owned-test"}, timeout_seconds=5)
        assert observed == [1, 0, 0, 0]
