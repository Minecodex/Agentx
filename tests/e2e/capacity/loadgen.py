"""Authenticated load generation; an accepted invocation must reach success."""

from __future__ import annotations

import asyncio
import math
import time
import uuid
from collections import Counter
from collections.abc import Awaitable, Callable
from contextlib import AsyncExitStack
from dataclasses import asdict, dataclass, field
from typing import Any

import httpx

TERMINAL = {"completed", "succeeded", "failed", "cancelled"}


@dataclass
class LoadSample:
    accepted_ms: float
    status: int
    invocation_id: str | None = None
    final_status: str | None = None
    error: str | None = None
    gateway_url: str | None = None


@dataclass
class LoadReport:
    samples: list[LoadSample] = field(default_factory=list)
    duration_seconds: float = 0
    connection_setup_ms: float = 0
    transport: str = "local-http"

    def summary(self) -> dict[str, Any]:
        latencies = sorted(sample.accepted_ms for sample in self.samples if sample.status == 202)
        total = len(self.samples)

        def percentile(fraction: float) -> float | None:
            return latencies[max(0, math.ceil(len(latencies) * fraction) - 1)] if latencies else None

        unexpected = sum(
            sample.status not in {202, 429}
            or sample.error is not None
            or (sample.status == 202 and sample.final_status not in {"succeeded", "completed"})
            for sample in self.samples
        )
        accepted = sum(sample.status == 202 and sample.invocation_id is not None for sample in self.samples)
        return {
            "totalRequests": total,
            "acceptedExecutions": accepted,
            "completedExecutions": sum(sample.final_status in {"succeeded", "completed"} for sample in self.samples),
            "acceptP50Ms": percentile(0.50),
            "acceptP95Ms": percentile(0.95),
            "acceptP99Ms": percentile(0.99),
            "nonSuccessRate": unexpected / total if total else None,
            "rejected429Rate": sum(sample.status == 429 for sample in self.samples) / total if total else None,
            "durationSeconds": self.duration_seconds,
            "connectionSetupMs": self.connection_setup_ms,
            "transport": self.transport,
            "gatewayTargets": dict(
                Counter(sample.gateway_url for sample in self.samples if sample.status == 202 and sample.gateway_url)
            ),
            "errors": [asdict(sample) for sample in self.samples if sample.error or sample.status not in {202, 429}][
                :20
            ],
        }


async def wait_terminal(
    client: httpx.AsyncClient, invocation: str, headers: dict[str, str], timeout: float = 180
) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        # Async callers poll once a second. Intake timing must not be
        # dominated by an immediate burst of 400 status reads per second.
        await asyncio.sleep(min(1, max(0, deadline - time.monotonic())))
        response = await client.get(f"/gateway/v1/invocations/{invocation}", headers=headers)
        response.raise_for_status()
        status = response.json()["status"]
        if status in TERMINAL:
            return status
    raise TimeoutError("accepted invocation did not reach a terminal state")


async def drive_invocations(
    base_urls: list[str],
    slug: str,
    api_keys: list[str],
    concurrent: int,
    duration_seconds: float,
    question: str = "capacity probe",
    *,
    max_requests: int | None = None,
    requests_per_second: float | None = None,
    transport: httpx.AsyncBaseTransport | None = None,
) -> LoadReport:
    if (
        not isinstance(base_urls, list)
        or not base_urls
        or any(not base for base in base_urls)
        or not isinstance(api_keys, list)
        or not api_keys
        or any(not key or key == "unset" for key in api_keys)
        or concurrent < 1
        or concurrent < len(base_urls)
        or duration_seconds <= 0
    ):
        raise ValueError("capacity load requires a real key, positive concurrency and duration")
    report = LoadReport()
    started_run = time.monotonic()
    deadline = started_run + duration_seconds
    run_key = uuid.uuid4().hex
    sequence = 0
    pacing_lock = asyncio.Lock()
    next_request = started_run

    async with AsyncExitStack() as stack:
        clients = [
            await stack.enter_async_context(
                httpx.AsyncClient(
                    base_url=base_urls[index % len(base_urls)],
                    timeout=30,
                    transport=transport,
                    limits=httpx.Limits(max_connections=1, max_keepalive_connections=1),
                )
            )
            for index in range(concurrent)
        ]
        # Record connection setup separately, then measure intake on the
        # persistent HTTP connections used by the steady-state callers.
        setup_started = time.monotonic()
        warmed = await asyncio.gather(
            *(
                clients[index].get(
                    "/health/ready", headers={"Authorization": f"Bearer {api_keys[index % len(api_keys)]}"}
                )
                for index in range(concurrent)
            )
        )
        for response in warmed:
            response.raise_for_status()
        report.connection_setup_ms = (time.monotonic() - setup_started) * 1000
        started_run = time.monotonic()
        deadline = started_run + duration_seconds
        next_request = started_run

        async def worker(index: int) -> None:
            nonlocal sequence, next_request
            headers = {"Authorization": f"Bearer {api_keys[index % len(api_keys)]}"}
            client = clients[index]
            while time.monotonic() < deadline:
                async with pacing_lock:
                    if max_requests is not None and sequence >= max_requests:
                        return
                    sequence += 1
                    request_number = sequence
                    scheduled = next_request
                    if requests_per_second:
                        next_request += 1 / requests_per_second
                if requests_per_second:
                    await asyncio.sleep(max(0, scheduled - time.monotonic()))
                    if time.monotonic() >= deadline:
                        return
                started = time.monotonic()
                sample = LoadSample(0, 0, gateway_url=str(client.base_url))
                try:
                    response = await client.post(
                        f"/gateway/v1/applications/{slug}/invocations",
                        headers={**headers, "Idempotency-Key": f"capacity-{run_key}-{request_number}"},
                        json={"input": {"message": f"{question} {request_number}"}, "responseMode": "async"},
                    )
                    sample.accepted_ms = (time.monotonic() - started) * 1000
                    sample.status = response.status_code
                    if response.status_code == 202:
                        sample.invocation_id = response.json()["id"]
                        sample.final_status = await wait_terminal(
                            client, sample.invocation_id, headers, timeout=duration_seconds
                        )
                    elif response.status_code != 429:
                        sample.error = f"unexpected HTTP {response.status_code} from the published capacity application"
                except (httpx.HTTPError, ValueError, KeyError, TimeoutError) as error:
                    sample.accepted_ms = (time.monotonic() - started) * 1000
                    sample.error = type(error).__name__
                report.samples.append(sample)

        await asyncio.gather(*(worker(index) for index in range(concurrent)))
    report.duration_seconds = time.monotonic() - started_run
    return report


async def probe_sse(
    base_urls: list[str],
    api_keys: list[str],
    invocation_id: str,
    *,
    on_all_connected: Callable[[], Awaitable[int]],
    connections: int = 200,
) -> dict[str, Any]:
    """Hold every stream open together, then test reconnect and terminal drain."""
    terminal_events = {"invocation.completed", "invocation.failed", "invocation.cancelled"}
    samples: list[dict[str, Any]] = []
    connected = 0
    all_connected = asyncio.Event()
    release = asyncio.Event()
    async with AsyncExitStack() as stack:
        clients = [
            await stack.enter_async_context(
                httpx.AsyncClient(base_url=base, timeout=60, limits=httpx.Limits(max_connections=connections + 5))
            )
            for base in base_urls
        ]

        async def replay(
            client: httpx.AsyncClient, key: str, after: int | None, stop_after_first: bool = False
        ) -> tuple[int, float, float]:
            nonlocal connected
            started = time.monotonic()
            last = after or 0
            kind = ""
            terminal_at = None
            async with client.stream(
                "GET",
                f"/gateway/v1/invocations/{invocation_id}/events",
                headers={"Authorization": f"Bearer {key}", **({"Last-Event-ID": str(after)} if after else {})},
            ) as response:
                response.raise_for_status()
                if stop_after_first:
                    connected += 1
                    if connected == connections:
                        all_connected.set()
                    await release.wait()
                async for line in response.aiter_lines():
                    if line.startswith("id:"):
                        cursor = int(line.partition(":")[2].strip())
                        if cursor <= last:
                            raise ValueError("SSE cursor repeated or went backwards")
                        last = cursor
                    elif line.startswith("event:"):
                        kind = line.partition(":")[2].strip()
                    elif not line:
                        if stop_after_first and last:
                            return last, time.monotonic() - started, 0
                        if kind in terminal_events:
                            terminal_at = time.monotonic()
                if terminal_at is None:
                    raise ValueError("SSE ended without a terminal event")
            return last, (terminal_at - started), time.monotonic() - terminal_at

        async def connection(index: int) -> None:
            sample: dict[str, Any] = {"connected": False, "reconnected": False}
            try:
                client = clients[index % len(clients)]
                key = api_keys[index % len(api_keys)]
                cursor, _, _ = await replay(client, key, None, stop_after_first=True)
                sample["connected"] = True
                _, catchup, drain = await replay(client, key, cursor)
                sample.update(reconnected=True, catchupSeconds=catchup, terminalDrainSeconds=drain)
            except (httpx.HTTPError, ValueError, TimeoutError) as error:
                sample["error"] = type(error).__name__
            samples.append(sample)

        tasks = [asyncio.create_task(connection(index)) for index in range(connections)]
        try:
            await asyncio.wait_for(all_connected.wait(), timeout=60)
            peak_live = await on_all_connected()
            if peak_live < connections:
                raise ValueError(f"only {peak_live} simultaneous SSE connections were observed")
            release.set()
            await asyncio.gather(*tasks)
        finally:
            release.set()
            for task in tasks:
                if not task.done():
                    task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
    successful = [sample for sample in samples if sample["reconnected"]]
    return {
        "connections": connections,
        "peakLiveConnections": peak_live,
        "connectFailureRate": sum(not sample["connected"] for sample in samples) / connections,
        "reconnectFailureRate": sum(not sample["reconnected"] for sample in samples) / connections,
        "catchupSeconds": max((sample["catchupSeconds"] for sample in successful), default=None),
        "terminalDrainSeconds": max((sample["terminalDrainSeconds"] for sample in successful), default=None),
        "errors": [sample for sample in samples if sample.get("error")][:20],
    }


if __name__ == "__main__":
    import json
    import sys

    report = asyncio.run(drive_invocations(**json.load(sys.stdin)))
    print(json.dumps(asdict(report)))
