"""Channel setup before deployment, required mappings, and real reply delivery."""

from __future__ import annotations

import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.product import test_channel_delivery as delivery
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]
im_mock = delivery.im_mock


def test_channel_configuration_and_reply_ui(installed_agentx, service_urls, im_mock):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        _access_token(control)
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_IM_MOCK_URL": im_mock["url"],
    }
    run_playwright(
        Path(installed_agentx["root"]), "channel-configuration", ("tests/channel-configuration.spec.ts",), environment
    )
    received = delivery._mock_received(installed_agentx["dependencies_namespace"], "session-channel-form")
    assert received, "The platform mock must receive the reply created through the channel UI"
    assert received[-1]["body"]["text"]["content"] == "回答：channel form message\n发送者：E2E"
