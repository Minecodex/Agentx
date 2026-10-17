"""Explicit version-owned working documents and publishing for API fixtures."""

import uuid

import httpx


def idempotent_headers(headers: dict | None = None) -> dict:
    return {**(headers or {}), "Idempotency-Key": str(uuid.uuid4())}


def fixture_draft(client: httpx.Client, workflow_id: str, headers: dict | None = None) -> dict:
    response = client.get(f"/api/v1/workflows/{workflow_id}/versions", headers=headers)
    response.raise_for_status()
    version = next(item for item in response.json() if item["versionNumber"] == 1)
    context = client.post(f"/api/v1/workflows/{workflow_id}/versions/{version['id']}/edit", headers=headers)
    context.raise_for_status()
    return context.json()["document"]


def fixture_draft_id(client: httpx.Client, workflow_id: str, headers: dict | None = None) -> str:
    return fixture_draft(client, workflow_id, headers)["id"]


def publish_fixture_version(
    client: httpx.Client, workflow: str, document: str, revision: int, headers: dict | None = None
) -> httpx.Response:
    working = client.get(f"/api/v1/workflows/{workflow}/drafts/{document}", headers=headers)
    working.raise_for_status()
    document = working.json()
    environments = client.get("/api/v1/environments", headers=headers)
    environments.raise_for_status()
    environment = next(item for item in environments.json() if item["code"] == "development")
    deployment = client.post(
        f"/api/v1/workflows/{workflow}/deployments",
        headers=idempotent_headers(headers),
        json={
            "environmentId": environment["id"],
            "workflowVersionId": document["versionId"],
            "contentSource": "draft" if document["publicationStatus"] == "published" else "version",
            "expectedRevision": revision,
        },
    )
    if not deployment.is_success:
        return deployment
    return client.get(
        f"/api/v1/workflows/{workflow}/versions/{deployment.json()['workflowVersionId']}", headers=headers
    )
