"""Real-model acceptance only consumes explicitly supplied test configuration."""

from __future__ import annotations

from collections.abc import Iterator

import pytest

from tests.e2e.product.live_model_config import ExistingSecret, model_configuration, model_secret


@pytest.fixture(scope="session")
def existing_model() -> dict:
    try:
        return model_configuration()
    except ValueError as error:
        pytest.fail(str(error), pytrace=False)


@pytest.fixture(scope="session")
def existing_model_secret(existing_model: dict) -> Iterator[ExistingSecret]:
    try:
        secret = model_secret()
    except ValueError as error:
        pytest.fail(str(error), pytrace=False)
    try:
        yield secret
    finally:
        secret.value = ""
