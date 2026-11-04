"""Resume retained management/page/history gates, keeping original AI work and budget."""

from __future__ import annotations

import json
import re
import shutil
from datetime import UTC, datetime
from pathlib import Path
from uuid import UUID

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.next_batch_support import get
from tests.e2e.runtime.test_agent_attachments import _control_mysql
from tests.e2e.support import run
from tools.scripts.release.evidence import source_tree_sha256


def retain_candidate(context, items, *, upgrade=False):
    run_id = context["run_id"]
    assert re.fullmatch(r"plan10-[a-z0-9-]+", run_id), "Only owned plan10 environments can resume"
    allowed = {
        "test_01_contract.py",
        "test_03_files_security.py",
        "test_05b_management.py",
        "test_05c_model_reasoning.py",
        "test_06_real_pages.py",
        "test_06b_employee_operations.py",
        "test_07_regression.py",
        "test_08_history.py",
    }
    test_root = Path(context["root"]) / "tests/e2e/product/plan10"
    assert items and all(
        Path(item.path).resolve().parent == test_root.resolve() and Path(item.path).name in allowed for item in items
    ), "Resume is limited to the explicit plan10 acceptance gates"
    directory = Path(context["artifact_dir"])
    prior = json.loads((directory / "candidate-identity.json").read_text())
    current_source = source_tree_sha256(Path(context["root"]))
    assert upgrade or prior["identity"]["sourceTreeSha256"] == current_source, (
        "A changed product candidate requires an explicit verified upgrade and affected regression gates"
    )
    for plane in ("control", "runtime", "dependencies"):
        namespace = context[f"{plane}_namespace"]
        assert namespace.startswith("agentx-e2e-") and namespace.endswith(run_id)
        run(("kubectl", "get", "namespace", namespace, "-o", "name"))
    pods = []
    for plane in ("control", "runtime", "dependencies"):
        pods.extend(run(("kubectl", "-n", context[f"{plane}_namespace"], "get", "pods", "-o", "json")).json()["items"])
    observed = {
        c["image"] for pod in pods for c in [*pod["spec"]["containers"], *pod["spec"].get("initContainers", [])]
    }
    # Completed bootstrap/doctor/migration Pods may already be garbage-collected.
    jobs = {"agentx-bootstrap", "agentx-doctor", "agentx-migrate"}
    running = {image for image in prior["images"] if image.rsplit("/", 1)[-1].split(":", 1)[0] not in jobs}
    assert running.issubset(observed), "The retained candidate images changed"
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
    archive = directory.with_name(directory.name + "-before-resume-" + stamp)
    shutil.copytree(directory, archive)
    browser = Path(context["root"]) / ".local/artifacts/playwright/helm-agentxctl" / run_id
    if browser.exists():
        shutil.copytree(browser, archive / "playwright")
    (directory / "resume-checkpoint.json").write_text(
        json.dumps(
            {
                "archive": str(archive),
                "identity": prior["identity"],
                "upgradeBeforeResume": upgrade,
                "requestedSourceTreeSha256": current_source,
            },
            indent=2,
        )
    )


def completed_page(directory: Path, kind: str) -> bool:
    """A business pass cannot override failed visual review of the same HTML."""
    path = directory / f"{kind}-complete.json"
    if not path.exists():
        return False
    result = json.loads(path.read_text())
    if not result["passed"]:
        return False
    review = directory / f"{kind}-visual-review.json"
    if review.exists():
        reviewed = json.loads(review.read_text())
        if reviewed["htmlHash"] == result["publication"]["htmlHash"] and not reviewed["passed"]:
            return False
    return True


def existing_suite(control, me, installed, urls, run_id):
    directory = Path(installed["artifact_dir"]) / "plan10"
    cases = json.loads((directory / "scenarios.json").read_text())
    assert len(cases) == 3 and {case["kind"] for case in cases} == {"expense", "quote", "file"}
    model = get(control, f"/models/aliases/{get(control, '/platform-ai/model-setting')['modelAliasId']}")
    assert model["alias"] == f"p8-plan10-{run_id}" and model["deploymentId"]

    def user(username):
        assert re.fullmatch(r"[a-z0-9-]+", username)
        tenant = str(UUID(me["companyId"]))
        identifier = _control_mysql(
            installed,
            f"SELECT BIN_TO_UUID(id) FROM users WHERE tenant_id=UUID_TO_BIN('{tenant}') AND username='{username}'",  # noqa: S608 — validated fixture username and tenant UUID.
        )
        assert identifier
        password = f"P7-next-private-{username}"
        logged = control.post("/api/v1/auth/login", auth=None, json={"username": username, "password": password})
        assert logged.status_code == 200, logged.text
        return {
            "id": identifier,
            "username": username,
            "password": Secret(password),
            "token": Secret(logged.json()["accessToken"]),
        }

    diagnostics = {}
    for case in cases:
        app = get(control, f"/applications/{case['applicationId']}")
        assert app["slug"] == case["slug"] and run_id in app["name"]
        previous = directory / f"{case['kind']}-authoring.json"
        if previous.exists():
            binding = json.loads(previous.read_text())["binding"]
            draft = get(control, f"/applications/{case['applicationId']}/ui/drafts/{binding['uiDraftId']}")
            stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
            prefix = directory / f"{case['kind']}-before-resume-{stamp}"
            prefix.with_suffix(".html").write_text(draft["html"])
            entries, cursor = [], 0
            while True:
                page = get(control, f"/design-agent/sessions/{binding['id']}/entries?after={cursor}")
                entries.extend(page["items"])
                if len(page["items"]) < 200:
                    break
                assert page["nextCursor"] > cursor
                cursor = page["nextCursor"]
            prefix.with_suffix(".json").write_text(
                json.dumps({"binding": binding, "draft": draft, "entries": entries}, ensure_ascii=False, indent=2)
            )
            draft_id = UUID(binding["uiDraftId"]).hex
            checked = _control_mysql(
                installed,
                f"SELECT HEX(CAST(report_json AS CHAR CHARACTER SET utf8mb4)) FROM application_ui_checks WHERE ui_draft_id=UNHEX('{draft_id}') ORDER BY checked_at DESC LIMIT 1",  # noqa: S608 — normalized owned draft UUID.
            )
            diagnostics[case["kind"]] = json.loads(bytes.fromhex(checked).decode())["errors"] if checked else []
        key = post(control, f"/applications/{case['applicationId']}/api-keys", {"name": "Resumed page acceptance"})
        case.update(keyId=key["id"], secret=Secret(key["secret"]))
    return {
        "control": control,
        "directory": directory,
        "cases": cases,
        "model": model,
        "me": me,
        "employee": user(f"batch-employee-{run_id}"),
        "other": user(f"batch-other-{run_id}"),
        "users": {actor: user(f"p9-{actor}-{run_id}") for actor in ("manager", "finance")},
        "installed": installed,
        "urls": urls,
        "runId": run_id,
        "resumed": True,
        "resumeDiagnostics": diagnostics,
    }
