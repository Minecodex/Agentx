"""Explicit core PR coverage and hosted release scope.

Independent acceptance keeps every test, including real providers and capacity.
"""

from __future__ import annotations

import pytest

PR_TESTS = {
    "infrastructure/test_installation.py::test_all_planes_are_ready",
    "infrastructure/test_installation.py::test_installed_pods_use_every_configured_release_image",
    "infrastructure/test_installation.py::test_bootstrap_is_idempotent",
    "infrastructure/test_mysql_readiness.py::test_mysql_readiness_rejects_wrong_credentials",
    "gateway/test_gateway.py::test_runtime_public_and_internal_services_exist",
    "publishing/test_release_boundaries.py::test_control_and_runtime_have_independent_releases",
    "runtime/test_runtime.py::test_runtime_workloads_are_available",
    "runtime/test_runtime.py::test_loop_checkpoint_recovers_after_workflow_runtime_restart",
    "runtime/test_agent_sessions.py::test_p305_invocation_sessions_are_isolated_and_entries_are_durable",
    "runtime/test_agent_sessions.py::test_p305_application_session_continues_across_gateway_executions",
    "product/test_channel_delivery.py::test_l1_reply_delivered_and_dead_letter_replay",
    "product/test_channel_delivery.py::test_l2_reply_node_delivers_to_trigger_conversation",
    "product/test_model_streaming.py::test_stream_deltas_arrive_before_terminal_in_order",
    "product/test_model_streaming.py::test_vision_attachment_reaches_model_as_native_parts",
    "product/test_model_streaming.py::test_studio_debug_has_an_independent_durable_delta_cursor",
    "product/test_playwright.py::test_product_playwright_suite",
    "product/test_studio_builder.py::test_plan8_layout_protocol",
    "product/test_web_asset_loading.py::test_workflow_creation_and_chunk_recovery",
    "product/test_workflow_version_canvases.py::test_workflow_version_canvases",
    "observability/test_observability.py::test_observability_is_available",
    "security/test_isolation.py::test_each_namespace_has_default_deny",
    "security/test_workload_credentials.py::test_serving_workload_uses_dedicated_service_account",
    "security/test_workload_credentials.py::test_workloads_only_reference_plane_secrets",
    "security/test_attack_matrix.py::test_idempotency_key_replay_with_different_input_is_rejected",
    "security/test_attack_matrix.py::test_unauthenticated_invocation_is_refused",
}


def select_ci_profile(config: pytest.Config, items: list[pytest.Item]) -> None:
    profile = config.getoption("--ci-profile")
    if profile == "independent":
        return
    available = {item.nodeid.replace("\\", "/").removeprefix("tests/e2e/").split("[", 1)[0] for item in items}
    if profile == "pr" and (missing := PR_TESTS - available):
        raise pytest.UsageError(f"Core PR scenarios missing from collection: {sorted(missing)}")
    selected, deselected = [], []
    for item in items:
        if "existing_model_secret" in item.fixturenames and not item.get_closest_marker("live_model"):
            raise pytest.UsageError(f"Real credential dependency must be declared live_model: {item.nodeid}")
        node = item.nodeid.replace("\\", "/").removeprefix("tests/e2e/").split("[", 1)[0]
        special_environment = any(item.get_closest_marker(mark) for mark in ("capacity", "live_model"))
        include = not special_environment and (profile == "release" or node in PR_TESTS)
        (selected if include else deselected).append(item)
    if not selected:
        raise pytest.UsageError(f"No scenarios collected for CI profile {profile}")
    config.hook.pytest_deselected(items=deselected)
    items[:] = selected
