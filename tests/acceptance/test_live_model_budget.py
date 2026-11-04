"""Real money admission, uncertainty and explicit credential invariants."""

import os
from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal

import pytest

from tests.e2e.product.live_model_budget import BudgetError, ModelBudget
from tests.e2e.product.live_model_config import model_configuration, model_secret


def payload():
    return {"model": "glm-5.3-flash", "messages": [{"role": "user", "content": "test"}], "max_tokens": 100}


def budget(tmp_path, limit="0.001"):
    return ModelBudget(tmp_path / "budget.json", limit=Decimal(limit), input_price=Decimal(1), output_price=Decimal(2))


def test_concurrent_admission_cannot_exceed_shared_budget(tmp_path):
    ledger = budget(tmp_path)

    def admit(_):
        try:
            return ledger.reserve(payload())
        except BudgetError:
            return None

    with ThreadPoolExecutor(max_workers=12) as pool:
        accepted = list(filter(None, pool.map(admit, range(40))))
    assert len(accepted) == 1
    snapshot = ledger.snapshot()
    assert Decimal(snapshot["spentUsd"]) + Decimal(snapshot["heldUsd"]) <= ledger.limit


def test_unknown_reservation_survives_restart_and_cannot_be_released(tmp_path):
    ledger = budget(tmp_path)
    identifier = ledger.reserve(payload())
    restarted = budget(tmp_path)
    assert restarted.snapshot()["pending"][identifier]["state"] == "unknown"
    with pytest.raises(BudgetError, match="Unknown provider outcomes"):
        restarted.unsent(identifier)
    with pytest.raises(BudgetError, match="EXHAUSTED"):
        restarted.reserve(payload())


def test_known_usage_settles_once_and_releases_unused_reservation(tmp_path):
    ledger = budget(tmp_path)
    identifier = ledger.reserve(payload())
    ledger.settle(identifier, {"prompt_tokens": 10, "completion_tokens": 5})
    assert Decimal(ledger.snapshot()["spentUsd"]) == Decimal("0.00002")
    assert ledger.snapshot()["heldUsd"] == "0"
    with pytest.raises(BudgetError, match="settled reservation"):
        ledger.settle(identifier, {"prompt_tokens": 10, "completion_tokens": 5})
    assert Decimal(ledger.snapshot()["spentUsd"]) == Decimal("0.00002")


def test_missing_usage_retains_worst_case_and_provider_overrun_halts(tmp_path):
    ledger = budget(tmp_path, "1")
    identifier = ledger.reserve(payload())
    with pytest.raises(BudgetError, match="retained"):
        ledger.settle(identifier, {})
    assert ledger.snapshot()["pending"][identifier]["state"] == "unknown"
    second = ledger.reserve(payload())
    with pytest.raises(BudgetError, match="exceeded"):
        ledger.settle(second, {"prompt_tokens": 10, "completion_tokens": 101})
    with pytest.raises(BudgetError, match="EXHAUSTED"):
        ledger.reserve(payload())


def test_no_model_switch_or_unbounded_output_can_reach_provider(tmp_path):
    ledger = budget(tmp_path)
    for change in ({"model": "another-model"}, {"max_tokens": None}, {"max_tokens": 16385}, {"max_tokens": True}):
        with pytest.raises(BudgetError):
            ledger.reserve({**payload(), **change})
    assert ledger.snapshot()["requests"] == 0


def test_configuration_is_explicit_and_never_discovers_live_cluster():
    with pytest.raises(ValueError, match="BASE_URL"):
        model_configuration({})
    environment = {
        "AGENTX_E2E_LLM_BASE_URL": "https://api.apimart.ai/v1",
        "AGENTX_E2E_LLM_MODEL": "glm-5.3-flash",
        "AGENTX_E2E_LLM_INPUT_USD_PER_MILLION": "0.06",
        "AGENTX_E2E_LLM_OUTPUT_USD_PER_MILLION": "0.20",
    }
    selected = model_configuration(environment)
    assert selected["modelName"] == "glm-5.3-flash"
    assert "secretRef" not in selected
    with pytest.raises(ValueError, match="exactly one"):
        model_secret(environment)
    secret = model_secret({**environment, "AGENTX_E2E_LLM_API_KEY": "  synthetic-acceptance-value\u2028"})
    assert secret.value == "synthetic-acceptance-value"
    assert "synthetic-acceptance-value" not in repr(secret)


def test_secret_file_requires_private_permissions_and_no_ambiguous_source(tmp_path):
    path = tmp_path / "credential"
    path.write_text("synthetic-test-value")
    path.chmod(0o644)
    if os.name != "nt":
        with pytest.raises(ValueError, match="owner"):
            model_secret({"AGENTX_E2E_LLM_API_KEY_FILE": str(path)})
    path.chmod(0o600)
    assert model_secret({"AGENTX_E2E_LLM_API_KEY_FILE": str(path)}).value == "synthetic-test-value"
    with pytest.raises(ValueError, match="exactly one"):
        model_secret({"AGENTX_E2E_LLM_API_KEY_FILE": str(path), "AGENTX_E2E_LLM_API_KEY": "other"})


def test_request_cap_survives_restart_and_cannot_be_raised(tmp_path):
    path = tmp_path / "capped.json"
    config = {"limit": Decimal(30), "input_price": Decimal("0.06"), "output_price": Decimal("0.2"), "max_requests": 2}
    ledger = ModelBudget(path, **config)
    first = ledger.reserve(payload())
    ledger.settle(first, {"prompt_tokens": 1, "completion_tokens": 1})
    ledger.reserve(payload())
    restarted = ModelBudget(path, **config)
    with pytest.raises(BudgetError, match="EXHAUSTED"):
        restarted.reserve(payload())
    with pytest.raises(BudgetError, match="identity differs"):
        ModelBudget(path, **{**config, "max_requests": 3})
    assert restarted.snapshot()["requests"] == 2
