"""Mem0 write, recall, filtering and deletion with actual CPU vectors."""

import json

import httpx
import pytest

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_mem0_writes_recalls_isolates_and_deletes_real_cpu_vectors(cpu_memory_service):
    client = cpu_memory_service["client"]
    subject = "p7-cpu-memory-owner"
    created = client.post(
        "/memories",
        json={
            "messages": [{"role": "user", "content": "我的项目上线代号是水星蓝桥20261008。"}],
            "user_id": subject,
            "infer": False,
        },
    )
    assert created.status_code == 200, created.text
    results = created.json()["results"]
    assert len(results) == 1, results
    memory_id = results[0]["id"]

    def search(user_id):
        response = client.post(
            "/search", json={"query": "我的项目上线代号是什么?", "filters": {"user_id": user_id}, "top_k": 5}
        )
        assert response.status_code == 200, response.text
        return response.json()["results"]

    recalled = search(subject)
    assert any("水星蓝桥20261008" in item["memory"] for item in recalled), recalled
    assert search("p7-cpu-memory-other-user") == []
    deleted = client.delete(f"/memories/{memory_id}")
    assert deleted.status_code == 200, deleted.text
    assert search(subject) == []
    report = {
        "memoryId": memory_id,
        "dimensions": 512,
        "write": "passed",
        "recall": "passed",
        "userFilterIsolation": "passed",
        "delete": "passed",
        "cloudLlmInferenceTested": False,
    }
    (cpu_memory_service["directory"] / "results.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    )


def test_mem0_configuration_requires_authentication(cpu_memory_service):
    response = httpx.post(cpu_memory_service["client"].base_url.join("/configure"), json={}, timeout=10)
    assert response.status_code == 401, response.text
