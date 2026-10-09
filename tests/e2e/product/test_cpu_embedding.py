"""Real CPU vector shape, semantics and authentication checks."""

import json
import math
from typing import Any

import httpx
import pytest

from tests.e2e.product.cpu_embedding_support import MODEL

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def _embeddings(service: dict[str, Any], texts: list[str]) -> list[list[float]]:
    response = httpx.post(
        service["url"] + "/embeddings",
        headers={"Authorization": f"Bearer {service['apiKey']}"},
        json={"model": MODEL, "input": texts, "encoding_format": "float", "dimensions": 512},
        timeout=45,
    )
    assert response.status_code == 200, response.text
    payload = response.json()
    assert [item["index"] for item in payload["data"]] == list(range(len(texts)))
    assert payload["usage"]["prompt_tokens"] > 0
    return [item["embedding"] for item in payload["data"]]


def test_cpu_embedding_has_real_vectors_and_chinese_semantic_order(cpu_embedding_service):
    service = cpu_embedding_service
    vectors = _embeddings(
        service,
        ["水星计划的上线日期是什么?", "水星计划将在2026年11月5日正式上线。", "小猫喜欢在窗边晒太阳。"],
    )
    assert all(len(vector) == 512 and all(math.isfinite(value) for value in vector) for vector in vectors)
    assert all(len({round(value, 6) for value in vector}) > 100 for vector in vectors)

    def cosine(left, right):
        return sum(a * b for a, b in zip(left, right, strict=True)) / (
            math.sqrt(sum(value * value for value in left)) * math.sqrt(sum(value * value for value in right))
        )

    relevant = cosine(vectors[0], vectors[1])
    unrelated = cosine(vectors[0], vectors[2])
    assert relevant > unrelated + 0.1, (relevant, unrelated)
    report = {
        "model": MODEL,
        "dimensions": 512,
        "image": service["receipt"]["image"],
        "relevantSimilarity": relevant,
        "unrelatedSimilarity": unrelated,
        "realCpuInference": True,
    }
    (service["directory"] / "semantic-results.json").write_text(json.dumps(report, indent=2) + "\n")


def test_cpu_embedding_requires_its_service_key(cpu_embedding_service):
    response = httpx.post(
        cpu_embedding_service["url"] + "/embeddings",
        headers={"Authorization": "Bearer invalid-embedding-credential"},
        json={"model": MODEL, "input": "认证失败时应拒绝生成向量"},
        timeout=15,
    )
    assert response.status_code in (401, 403), response.text
