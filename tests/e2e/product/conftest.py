"""Share live provider fixtures once per run, including across test modules."""

from tests.e2e.product import cpu_embedding_support as embedding
from tests.e2e.product import cpu_memory_support as memory
from tests.e2e.product import live_evaluation_support as evaluation
from tests.e2e.product import live_provider_support as providers
from tests.e2e.product import live_text_support as live
from tests.e2e.product import remaining_evaluation_support as boundary_evaluations
from tests.e2e.product import remaining_support as boundary

cpu_embedding_service = embedding.cpu_embedding_service
cpu_memory_service = memory.cpu_memory_service
live_kimi_secret = live.live_kimi_secret
live_application = live.live_application
live_comparison = evaluation.live_comparison
live_providers = providers.live_providers

boundary_state = boundary.boundary_state
boundary_gateway_metrics = boundary.boundary_gateway_metrics
five_comparisons = boundary_evaluations.five_comparisons
