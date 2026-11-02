"""Renew the test user's short-lived token before sending a business request."""

from __future__ import annotations

import base64
import json
import time
from typing import TYPE_CHECKING

import httpx

if TYPE_CHECKING:
    from tests.e2e.product.live_text_support import Secret


class UserTokenAuth(httpx.Auth):
    def __init__(self, control_url: str, token: Secret, *, username: str, password: Secret):
        self.control_url = control_url
        self.token = token
        self.issued_tokens = {token.value}
        self.username = username
        self.password = password

    def _login(self) -> str:
        with httpx.Client(base_url=self.control_url, timeout=60, trust_env=False) as login:
            response = login.post(
                "/api/v1/auth/login", json={"username": self.username, "password": self.password.value}
            )
            response.raise_for_status()
            return response.json()["accessToken"]

    def refresh_if_needed(self) -> None:
        self.issued_tokens.add(self.token.value)
        payload = self.token.value.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        if time.time() + 30 >= claims["exp"]:
            value = self._login()
            renewed_payload = value.split(".")[1]
            renewed = json.loads(base64.urlsafe_b64decode(renewed_payload + "=" * (-len(renewed_payload) % 4)))
            assert all(renewed[key] == claims[key] for key in ("sub", "tenantId", "iss")), (
                "Test credential renewal changed the caller identity"
            )
            self.token.value = value
            self.issued_tokens.add(value)

    def auth_flow(self, request):
        self.issued_tokens.add(self.token.value)
        authorization = request.headers.get("Authorization")
        if authorization and authorization.removeprefix("Bearer ") not in self.issued_tokens:
            # Mixed callers keep their explicitly supplied application key or
            # negative-test credential; do not replace it with this test user.
            yield request
            return
        self.refresh_if_needed()
        request.headers["Authorization"] = f"Bearer {self.token.value}"
        yield request
