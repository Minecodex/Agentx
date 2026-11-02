"""Explicit real-provider test configuration; never discovers production credentials."""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from decimal import Decimal, InvalidOperation
from pathlib import Path
from urllib.parse import urlsplit


@dataclass
class ExistingSecret:
    value: str = field(default="", repr=False)


def _price(environment: dict[str, str], name: str) -> str:
    value = environment.get(name, "")
    try:
        number = Decimal(value)
    except InvalidOperation:
        raise ValueError(f"{name} must contain an explicit USD price per million tokens") from None
    if not number.is_finite() or number <= 0:
        raise ValueError(f"{name} must be finite and positive")
    return str(number)


def model_configuration(environment: dict[str, str] | None = None) -> dict:
    environment = dict(os.environ) if environment is None else environment
    endpoint = environment.get("AGENTX_E2E_LLM_BASE_URL", "").rstrip("/")
    parsed = urlsplit(endpoint)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError("AGENTX_E2E_LLM_BASE_URL must be an explicit HTTPS endpoint without credentials")
    name = environment.get("AGENTX_E2E_LLM_MODEL", "")
    if name != "glm-5.3-flash":
        raise ValueError("This acceptance run requires AGENTX_E2E_LLM_MODEL=glm-5.3-flash")
    raw_parameters = environment.get("AGENTX_E2E_LLM_PARAMETERS", "{}")
    try:
        parameters = json.loads(raw_parameters)
    except ValueError:
        raise ValueError("AGENTX_E2E_LLM_PARAMETERS must be a JSON object") from None
    if not isinstance(parameters, dict):
        raise ValueError("AGENTX_E2E_LLM_PARAMETERS must be a JSON object")
    if any(key in parameters for key in ("api_key", "headers", "base_url", "model")):
        raise ValueError("Provider identity and credentials cannot be overridden in model parameters")
    return {
        "alias": name,
        "endpoint": endpoint,
        "modelName": name,
        "providerType": "openai_compatible",
        "defaultParameters": parameters,
        "maxInputTokens": 64000,
        "maxOutputTokens": 16000,
        "inputPrice": _price(environment, "AGENTX_E2E_LLM_INPUT_USD_PER_MILLION"),
        "outputPrice": _price(environment, "AGENTX_E2E_LLM_OUTPUT_USD_PER_MILLION"),
        "currency": "USD",
    }


def model_secret(environment: dict[str, str] | None = None) -> ExistingSecret:
    environment = dict(os.environ) if environment is None else environment
    inline = environment.get("AGENTX_E2E_LLM_API_KEY", "")
    filename = environment.get("AGENTX_E2E_LLM_API_KEY_FILE", "")
    if bool(inline) == bool(filename):
        raise ValueError("Set exactly one of AGENTX_E2E_LLM_API_KEY and AGENTX_E2E_LLM_API_KEY_FILE")
    if filename:
        path = Path(filename)
        if not path.is_file():
            raise ValueError("The explicitly configured test credential file does not exist")
        if os.name != "nt" and path.stat().st_mode & 0o077:
            raise ValueError("The test credential file must only be accessible by its owner")
        inline = path.read_text(encoding="utf-8")
    value = inline.strip()
    if not value or any(character.isspace() for character in value):
        raise ValueError("The configured provider credential must be a single nonempty value")
    return ExistingSecret(value)
