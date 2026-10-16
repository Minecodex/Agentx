"""Explicit Draft selection for fixtures that intentionally create one canvas."""

import httpx


def fixture_draft(client: httpx.Client, workflow_id: str, headers: dict | None = None) -> dict:
    response = client.get(f"/api/v1/workflows/{workflow_id}/drafts", headers=headers)
    response.raise_for_status()
    drafts = response.json()
    assert len(drafts) == 1, "This fixture must name a Draft explicitly when it creates multiple canvases"
    return drafts[0]


def fixture_draft_id(client: httpx.Client, workflow_id: str, headers: dict | None = None) -> str:
    return fixture_draft(client, workflow_id, headers)["id"]
