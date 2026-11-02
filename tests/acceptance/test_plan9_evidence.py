"""Paid usage must exclude deterministic fixtures and other environments."""

import pytest

from tests.e2e.product.plan9.evidence import select_paid_bindings


def test_paid_usage_scope_requires_the_frozen_model_and_this_runs_metered_endpoint():
    namespace = "agentx-e2e-deps-test"
    paid = {
        "resourceId": "live",
        "model": "glm-5.3-flash",
        "source": "design_binding",
        "endpoint": f"https://plan9-model-relay.{namespace}.svc:8443/v1",
    }
    rows = [
        paid,
        {**paid, "resourceId": "fake", "model": "echo-studio-builder"},
        {**paid, "resourceId": "other-run", "endpoint": paid["endpoint"].replace(namespace, "other")},
        {**paid, "resourceId": "unmetered", "endpoint": "https://api.apimart.ai/v1"},
        {**paid, "resourceId": "wrong-port", "endpoint": paid["endpoint"].replace("8443", "9443")},
    ]
    selected = select_paid_bindings(rows, namespace)
    assert [row["resourceId"] for row in selected] == ["live"]
    assert "endpoint" not in selected[0]
    with pytest.raises(AssertionError):
        select_paid_bindings(
            [{**paid, "endpoint": paid["endpoint"].replace("https://", "https://user:secret@")}], namespace
        )
