"""Capacity load generation (plan7 P7-D4).

Drives concurrent gateway invocations against a published application and
records per-request accept latencies. Kept dependency-free (httpx from the
test group) so the capacity window only needs the standard e2e environment.
"""

from __future__ import annotations

import asyncio
import json
import time
from dataclasses import dataclass, field
from typing import Any

import httpx


@dataclass
class LoadSample:
    started_at: float
    accepted_ms: float
    status: int


@dataclass
class LoadReport:
    samples: list[LoadSample] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)

    def summary(self) -> dict[str, Any]:
        latencies = sorted(sample.accepted_ms for sample in self.samples)
        total = len(latencies)

        def percentile(fraction: float) -> float:
            if not total:
                return 0.0
            index = min(total - 1, int(total * fraction))
            return latencies[index]

        return {
            "totalRequests": total,
            "acceptP50Ms": percentile(0.50),
            "acceptP95Ms": percentile(0.95),
            "acceptP99Ms": percentile(0.99),
            "nonSuccessRate": (sum(1 for sample in self.samples if sample.status >= 500) / total) if total else 0.0,
            "rejected429": sum(1 for sample in self.samples if sample.status == 429),
            "errors": self.errors[:20],
        }


async def drive_invocations(
    base_url: str,
    slug: str,
    api_key: str,
    concurrent: int,
    duration_seconds: float,
    question: str = "capacity probe",
) -> LoadReport:
    report = LoadReport()
    client = httpx.AsyncClient(base_url=base_url, timeout=30)
    deadline = time.monotonic() + duration_seconds
    semaphore = asyncio.Semaphore(concurrent)

    async def worker(index: int) -> None:
        sequence = 0
        while time.monotonic() < deadline:
            sequence += 1
            async with semaphore:
                started = time.monotonic()
                try:
                    response = await client.post(
                        f"/gateway/v1/applications/{slug}/invocations",
                        headers={
                            "Authorization": f"Bearer {api_key}",
                            "Idempotency-Key": f"capacity-{index}-{started}-{sequence}",
                        },
                        json={"input": {"message": f"{question} {index}/{sequence}"}, "responseMode": "async"},
                    )
                    report.samples.append(
                        LoadSample(
                            started_at=started,
                            accepted_ms=(time.monotonic() - started) * 1000,
                            status=response.status_code,
                        )
                    )
                except Exception as error:
                    report.errors.append(str(error))

    workers = [asyncio.create_task(worker(index)) for index in range(concurrent)]
    await asyncio.gather(*workers)
    await client.aclose()
    return report


def write_report(directory, name: str, payload: dict[str, Any]) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{name}.json"
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
