"""Share live provider fixtures once per run, including across test modules."""

import httpx
import pytest

from tests.e2e.product import cpu_embedding_support as embedding
from tests.e2e.product import cpu_memory_support as memory
from tests.e2e.product import existing_model_support as source_model
from tests.e2e.product import live_evaluation_support as evaluation
from tests.e2e.product import live_provider_support as providers
from tests.e2e.product import live_text_support as live
from tests.e2e.product import remaining_evaluation_support as boundary_evaluations
from tests.e2e.product import remaining_support as boundary
from tests.e2e.product import studio_builder_support as studio
from tests.e2e.runtime.test_agent_attachments import _access_token

cpu_embedding_service = embedding.cpu_embedding_service
cpu_memory_service = memory.cpu_memory_service
existing_model = source_model.existing_model
existing_model_secret = source_model.existing_model_secret
live_application = live.live_application
live_comparison = evaluation.live_comparison
live_providers = providers.live_providers

boundary_state = boundary.boundary_state
boundary_gateway_metrics = boundary.boundary_gateway_metrics
five_comparisons = boundary_evaluations.five_comparisons
studio_provider = studio.studio_provider
studio_models = studio.studio_models
studio_users = studio.studio_users


def _refresh_shared_auth(request):
    names = request.fixturenames
    if not {"live_application", "live_providers", "boundary_state"}.intersection(names):
        return
    urls = request.getfixturevalue("service_urls")
    with httpx.Client(base_url=urls["web"], timeout=60) as control:
        token, _ = _access_token(control)
    if "live_application" in names:
        request.getfixturevalue("live_application")["token"].value = token
    if "live_providers" in names:
        request.getfixturevalue("live_providers")["control"].headers["Authorization"] = f"Bearer {token}"
    if "boundary_state" in names:
        state = request.getfixturevalue("boundary_state")
        state["token"].value = token
        state["control"].headers["Authorization"] = f"Bearer {token}"


@pytest.fixture(scope="module", autouse=True)
def refresh_shared_auth_before_module(request):
    # Session-scoped resources outlive the real 15-minute access token. Refresh
    # before module fixtures use them; never extend the production token TTL.
    _refresh_shared_auth(request)


@pytest.fixture(autouse=True)
def refresh_shared_auth_before_test(request):
    _refresh_shared_auth(request)
