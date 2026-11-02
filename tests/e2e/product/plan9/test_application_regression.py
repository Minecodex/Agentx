"""Reuse file/evaluation assertions, original order and the sole provider they actually use."""

import pytest

from tests.e2e.product import test_application_evaluations as evaluations
from tests.e2e.product import test_application_files as files

pytestmark = [pytest.mark.cluster, pytest.mark.product]
application_testing = evaluations.application_testing

test_batch_contracts_copy_revision_recovery_and_model_isolation = (
    evaluations.test_batch_contracts_copy_revision_recovery_and_model_isolation
)
test_application_scopes_and_platform_model_reader = evaluations.test_application_scopes_and_platform_model_reader
test_selected_case_export_uses_existing_view_permission_and_revision = (
    evaluations.test_selected_case_export_uses_existing_view_permission_and_revision
)
test_invocation_validation_and_atomic_dataset_authoring = (
    evaluations.test_invocation_validation_and_atomic_dataset_authoring
)
test_dataset_file_references_and_snapshot_completeness = (
    evaluations.test_dataset_file_references_and_snapshot_completeness
)
test_application_testing_desktop = evaluations.test_application_testing_desktop
test_application_output_file_download_authorization = files.test_application_output_file_download_authorization


@pytest.fixture(scope="session")
def e2e_providers(studio_provider):
    # Only echo_mcp is used by these assertions. Reuse the existing deployment
    # rather than starting unrelated RAGFlow/LightRAG/Mem0 services.
    return {"echo_mcp": studio_provider.removesuffix("/v1")}
