"""Redis backlog, pending work, unavailable cache and idempotent chat replay."""

from __future__ import annotations

import time

import httpx
import pytest

from tests.e2e.capacity.collector import parse_metrics, redis_command
from tests.e2e.capacity.fixtures import capacity_application
from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.remaining_support import report
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product]
STREAM = "agentx:v2:tasks:v1:model"
GROUP = "agentx:v2:workers:v1"


def scale(context, workload, replicas):
    namespace = context["runtime_namespace"]
    run(("kubectl", "-n", namespace, "scale", workload, f"--replicas={replicas}"), timeout=60)
    if replicas:
        run(("kubectl", "-n", namespace, "rollout", "status", workload, "--timeout=300s"), timeout=330)
    else:
        name = workload.split("/")[1]
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "wait",
                "--for=delete",
                "pod",
                "-l",
                f"app.kubernetes.io/name={name}",
                "--timeout=120s",
            ),
            timeout=150,
        )


def metric(metrics_url, name, predicate):
    deadline = time.monotonic() + 30
    while True:
        values = parse_metrics(httpx.get(f"{metrics_url}/metrics", timeout=5).text)
        if predicate(values.get(name, -1)):
            return values
        assert time.monotonic() < deadline, values
        time.sleep(0.5)


def test_redis_real_watermark_rejects_new_api_and_chat_preserves_replay(boundary_state, boundary_gateway_metrics):
    state = boundary_state
    context = state["context"]
    app = capacity_application(context, state["urls"]["web"], 1)
    key = Secret(app.pop("apiKey"))
    with httpx.Client(
        base_url=state["urls"]["runtime"], headers={"Authorization": f"Bearer {key.value}"}, timeout=30
    ) as gateway:
        raw = gateway.post(
            f"/gateway/v1/applications/{app['slug']}/invocations",
            headers={"Idempotency-Key": "redis-before-pressure"},
            json={"input": {"message": "before pressure"}, "responseMode": "async"},
        )
        assert raw.status_code == 202, raw.text
        deployments = get(state["control"], f"/applications/{app['applicationId']}/deployments")
        mapping_path = f"/applications/{app['applicationId']}/deployments/{deployments[0]['id']}/playground-config"
        config = get(state["control"], mapping_path)
        response = state["control"].put(
            f"/api/v1{mapping_path}",
            json={
                "expectedVersion": config["version"],
                "mapping": {
                    "questionInput": "message",
                    "fileInput": None,
                    "answerOutput": "answer",
                    "answerFilesOutput": None,
                },
            },
        )
        assert response.status_code in {200, 202}, response.text
        deadline = time.monotonic() + 90
        while get(state["control"], mapping_path)["publishStatus"] != "active":
            assert time.monotonic() < deadline
            time.sleep(0.5)
        session = gateway.post(
            f"/gateway/v1/applications/{app['slug']}/sessions",
            headers={"Idempotency-Key": "redis-session"},
            json={"title": None, "externalUserId": "redis-proof"},
        )
        assert session.status_code == 201, session.text
        chat_path = f"/gateway/v1/sessions/{session.json()['id']}/messages"
        chat_body = {"parts": [{"partType": "text", "content": "accepted before pressure"}]}
        accepted = gateway.post(chat_path, headers={"Idempotency-Key": "redis-chat-replay"}, json=chat_body)
        assert accepted.status_code == 202, accepted.text
        deadline = time.monotonic() + 90
        while gateway.get(f"/gateway/v1/invocations/{accepted.json()['id']}").json()["status"] not in {
            "succeeded",
            "completed",
        }:
            assert time.monotonic() < deadline
            time.sleep(0.5)
        scale(context, "deployment/workflow-worker", 0)
        try:
            redis_command(context, "XGROUP", "CREATE", STREAM, GROUP, "0-0", "MKSTREAM") if not redis_command(
                context, "EXISTS", STREAM
            ) else None
            baseline = int(_runtime_mysql(context, "SELECT COUNT(*) FROM application_invocations;"))
            inserted = redis_command(
                context,
                "EVAL",
                "local ids={} for i=1,2000 do ids[#ids+1]=redis.call('XADD',KEYS[1],'*','task','{}') end return ids",
                "1",
                STREAM,
            )
            values = metric(boundary_gateway_metrics, "agentx_redis_task_unread_items", lambda value: value >= 2000)
            responses = [
                gateway.post(
                    f"/gateway/v1/applications/{app['slug']}/invocations",
                    headers={"Idempotency-Key": "redis-overload-api"},
                    json={"input": {"message": "refuse"}},
                ),
                gateway.post(chat_path, headers={"Idempotency-Key": "redis-overload-chat"}, json=chat_body),
            ]
            assert all(
                r.status_code == 429
                and r.json()["code"] == "RUNTIME_ADMISSION_REJECTED"
                and int(r.headers["retry-after"]) > 0
                for r in responses
            ), [(r.status_code, r.text) for r in responses]
            assert int(_runtime_mysql(context, "SELECT COUNT(*) FROM application_invocations;")) == baseline
            replay = gateway.post(chat_path, headers={"Idempotency-Key": "redis-chat-replay"}, json=chat_body)
            assert replay.status_code == 202 and replay.json()["id"] == accepted.json()["id"], replay.text
            # Pending is work too; it must not disappear from the overload
            # budget when a consumer claims all unread messages.
            redis_command(
                context, "XREADGROUP", "GROUP", GROUP, "boundary-held", "COUNT", "2000", "STREAMS", STREAM, ">"
            )
            pending = metric(boundary_gateway_metrics, "agentx_redis_task_pending_items", lambda value: value >= 2000)
            assert (
                gateway.post(chat_path, headers={"Idempotency-Key": "redis-pending-chat"}, json=chat_body).status_code
                == 429
            )
            redis_command(context, "XACK", STREAM, GROUP, *inserted)
            redis_command(context, "XDEL", STREAM, *inserted)
            metric(boundary_gateway_metrics, "agentx_redis_task_unread_items", lambda value: value == 0)
            metric(boundary_gateway_metrics, "agentx_redis_task_pending_items", lambda value: value == 0)
            recovered = gateway.post(chat_path, headers={"Idempotency-Key": "redis-recovered-chat"}, json=chat_body)
            assert recovered.status_code == 202, recovered.text
            report(
                state,
                "remaining-redis-watermark",
                {
                    "unreadMetrics": values,
                    "pendingMetrics": pending,
                    "apiAndChatRefused": True,
                    "noRejectedReceipt": True,
                    "chatReplayPreserved": True,
                    "intakeRecovered": True,
                },
            )
        finally:
            if "inserted" in locals():
                redis_command(context, "XACK", STREAM, GROUP, *inserted)
                redis_command(context, "XDEL", STREAM, *inserted)
            scale(context, "deployment/workflow-worker", 1)


def test_redis_unavailable_refuses_and_recovers_without_accepting_work(boundary_state, boundary_gateway_metrics):
    state = boundary_state
    context = state["context"]
    app = capacity_application({**context, "run_id": f"{context['run_id']}-outage"}, state["urls"]["web"], 1)
    key = Secret(app.pop("apiKey"))
    with httpx.Client(
        base_url=state["urls"]["runtime"], headers={"Authorization": f"Bearer {key.value}"}, timeout=20
    ) as gateway:
        baseline = int(_runtime_mysql(context, "SELECT COUNT(*) FROM application_invocations;"))
        scale(context, "statefulset/runtime-redis", 0)
        try:
            refused = gateway.post(
                f"/gateway/v1/applications/{app['slug']}/invocations",
                headers={"Idempotency-Key": "redis-unavailable"},
                json={"input": {"message": "must not accept"}},
            )
            assert refused.status_code == 503 and refused.json()["code"] == "RUNTIME_STORAGE_UNAVAILABLE", refused.text
            metric(boundary_gateway_metrics, "agentx_redis_admission_available", lambda value: value == 0)
            assert int(_runtime_mysql(context, "SELECT COUNT(*) FROM application_invocations;")) == baseline
        finally:
            scale(context, "statefulset/runtime-redis", 1)
        metric(boundary_gateway_metrics, "agentx_redis_admission_available", lambda value: value == 1)
        recovered = gateway.post(
            f"/gateway/v1/applications/{app['slug']}/invocations",
            headers={"Idempotency-Key": "redis-after-unavailable"},
            json={"input": {"message": "recovered"}},
        )
        assert recovered.status_code == 202, recovered.text
        report(
            state, "remaining-redis-unavailable", {"failedClosed": True, "noRejectedReceipt": True, "recovered": True}
        )
