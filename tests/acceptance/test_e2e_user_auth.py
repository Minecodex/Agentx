"""Long acceptance runs renew before mutation and never replay a refusal."""

from __future__ import annotations

import base64
import json
import time

import httpx

from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.user_auth import UserTokenAuth


def fixture_token(expires):
    payload = base64.urlsafe_b64encode(json.dumps({"exp": expires}).encode()).decode().rstrip("=")
    return f"fixture.{payload}.signature"


def test_expired_user_token_is_renewed_before_each_business_mutation(monkeypatch):
    from tests.e2e.product import user_auth

    calls = []
    fresh = fixture_token(time.time() + 900)
    token = Secret(fixture_token(time.time() - 1))

    def login(client):
        calls.append("login")
        assert client.base_url.host == "control.invalid"
        assert client.base_url.scheme == "https"
        return fresh, {}

    def business(request):
        calls.append("business")
        assert request.method == "POST"
        assert request.headers["Authorization"] == f"Bearer {fresh}"
        return httpx.Response(201, json={"id": "owned-receipt"})

    monkeypatch.setattr(user_auth, "_access_token", login)
    with httpx.Client(
        base_url="https://control.invalid",
        transport=httpx.MockTransport(business),
        auth=UserTokenAuth("https://control.invalid", token),
    ) as control:
        assert control.post("/first", json={"input": "first"}).status_code == 201
        assert control.post("/second", json={"input": "second"}).status_code == 201
    assert calls == ["login", "business", "business"]
    assert token.value == fresh
    assert fresh not in repr(token)


def test_server_authentication_refusal_is_not_retried(monkeypatch):
    from tests.e2e.product import user_auth

    calls = []
    token = Secret(fixture_token(time.time() + 900))

    def unexpected_login(client):
        raise AssertionError("A fresh credential refusal must remain visible")

    def refused(request):
        calls.append(request.method)
        return httpx.Response(401, json={"code": "UNAUTHORIZED"})

    monkeypatch.setattr(user_auth, "_access_token", unexpected_login)
    with httpx.Client(
        base_url="https://control.invalid",
        transport=httpx.MockTransport(refused),
        auth=UserTokenAuth("https://control.invalid", token),
    ) as control:
        response = control.post("/mutation", json={"input": "once"})
    assert response.status_code == 401
    assert calls == ["POST"]


def test_explicit_application_key_is_not_replaced_by_the_test_user(monkeypatch):
    from tests.e2e.product import user_auth

    token = Secret(fixture_token(time.time() - 1))
    calls = []

    def unexpected_login(client):
        raise AssertionError("Application keys must keep their own caller identity")

    def gateway(request):
        calls.append(request.headers["Authorization"])
        return httpx.Response(202, json={"id": "application-receipt"})

    monkeypatch.setattr(user_auth, "_access_token", unexpected_login)
    with httpx.Client(
        base_url="https://gateway.invalid",
        transport=httpx.MockTransport(gateway),
        auth=UserTokenAuth("https://control.invalid", token),
    ) as client:
        response = client.post("/invocations", headers={"Authorization": "Bearer owned-application-key"})
    assert response.status_code == 202
    assert calls == ["Bearer owned-application-key"]
