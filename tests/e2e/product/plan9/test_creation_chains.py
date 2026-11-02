"""Four real creation chains; route runtime failures to Canvas, with shared attempt limits."""

import json
import subprocess
import time
from pathlib import Path

import pytest

from tests.e2e.product.plan9.authoring_inputs import verify_guidance
from tests.e2e.product.plan9.creation_chain_repair import repair_blueprint
from tests.e2e.product.plan9.creation_chains import prepare, publish_applications
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_real_pages import assemble_page_results
from tests.e2e.support import run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.product,
    pytest.mark.live_model,
    pytest.mark.usefixtures("plan9_business_protocol"),
]


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2))


def browser(suite, name, spec, environment):
    try:
        run_playwright(Path(suite["installed"]["root"]), name, (spec, "--workers=1"), environment)
        return {"exitCode": 0}
    except subprocess.CalledProcessError as error:
        # A failed real attempt remains evidence. Only the structured report
        # decides whether the remaining budget belongs to Page or Canvas AI.
        return {"exitCode": error.returncode}


def run_chain(suite, blueprint, persist):
    scene, started = blueprint["scene"], time.monotonic()
    workflow = {
        "scene": scene,
        "workflowId": blueprint["workflowId"],
        "passed": False,
        "attempts": [],
        "iterations": [],
    }
    page = {"scene": f"{scene}-chain", "round": 1, "passed": False, "attempts": [], "executions": []}
    directory = suite["directory"] / "creation-chains" / scene
    directory.mkdir(parents=True)
    environment = browser_environment(suite)
    try:
        for iteration in range(3):
            canvas_left, page_left = 3 - len(workflow["attempts"]), 3 - len(page["attempts"])
            if min(canvas_left, page_left) <= 0:
                break
            current = directory / f"iteration-{iteration}"
            current.mkdir()
            blueprints, report = current / "blueprints.json", current / "workflow-results.json"
            write(blueprints, [{**blueprint, "attemptLimit": canvas_left}])
            state = {"iteration": iteration}
            workflow["iterations"].append(state)
            state["canvasProcess"] = browser(
                suite,
                f"plan9-chain-{scene}-{iteration}-canvas",
                "tests/plan9-workflow-creation.spec.ts",
                {
                    **environment,
                    "AGENTX_E2E_PLAN9_WORKFLOW_BLUEPRINTS": str(blueprints),
                    "AGENTX_E2E_PLAN9_WORKFLOW_REPORT": str(report),
                },
            )
            generated = json.loads(report.read_text())[0] if report.exists() else {"passed": False, "attempts": []}
            assert 1 <= len(generated["attempts"]) <= canvas_left, generated
            workflow["attempts"].extend({**attempt, "iteration": iteration} for attempt in generated["attempts"])
            state["staticPassed"] = generated["passed"]
            persist(workflow, page)
            if not generated["passed"]:
                break
            state["canvasGuidance"] = verify_guidance(suite, generated["attempts"])
            workflow["publishedVersionId"] = generated["publishedVersionId"]
            scenarios = publish_applications(
                suite, [generated], iteration=iteration, directory=current, page_attempt_budget=page_left
            )
            pages = current / "page-results.json"
            state["pageProcess"] = browser(
                suite,
                f"plan9-chain-{scene}-{iteration}-page",
                "tests/plan9-real-pages.spec.ts",
                {
                    **environment,
                    "AGENTX_E2E_PLAN9_SCENARIOS": str(scenarios),
                    "AGENTX_E2E_PLAN9_REPORT": str(pages),
                    "AGENTX_E2E_PLAN9_ROUNDS": "1",
                },
            )
            rows = assemble_page_results(pages)
            assert len(rows) == 1, rows
            result = rows[0]
            assert 1 <= len(result["attempts"]) <= page_left, result
            state["pageGuidance"] = verify_guidance(
                suite, [{**attempt, "sessionId": result["sessionId"]} for attempt in result["attempts"]], page=True
            )
            if scene == "S08" and result["passed"]:
                assert any(row["consumedCount"] == 1 for row in state["canvasGuidance"])
                assert any(row["consumedCount"] == 1 for row in state["pageGuidance"])
            page["attempts"].extend(
                {
                    **attempt,
                    "iteration": iteration,
                    "workflowVersionId": generated["publishedVersionId"],
                    "draftId": result["draftId"],
                    "sessionId": result["sessionId"],
                }
                for attempt in result["attempts"]
            )
            page.setdefault("guidanceInputs", []).extend(result.get("guidanceInputs", []))
            page.update(
                {key: value for key, value in result.items() if key not in {"attempts", "elapsedMs", "guidanceInputs"}}
            )
            state.update(
                pagePassed=result["passed"], pageDraftId=result.get("draftId"), pageAttempts=len(result["attempts"])
            )
            persist(workflow, page)
            if result["passed"]:
                workflow["passed"] = True
                page.pop("workflowFailure", None)
                break
            failure = result.get("workflowFailure")
            if not failure or len(workflow["attempts"]) >= 3 or len(page["attempts"]) >= 3:
                break
            blueprint = repair_blueprint(suite, blueprint, generated, failure, current, 3 - len(workflow["attempts"]))
    except Exception as error:
        workflow["orchestrationError"] = f"{type(error).__name__}: {error}"
    finally:
        page["elapsedMs"] = round((time.monotonic() - started) * 1000)
        persist(workflow, page)
    return workflow, page


def test_four_canvas_to_page_creation_chains(plan9_suite):
    suite, workflows, pages = plan9_suite, [], []
    blueprints = json.loads(prepare(suite).read_text())

    def persist(workflow, page):
        write(suite["directory"] / "workflow-ai-results.json", [*workflows, workflow])
        write(suite["directory"] / "creation-chain-page-results.json", [*pages, page])

    for blueprint in blueprints:
        workflow, page = run_chain(suite, blueprint, persist)
        workflows.append(workflow)
        pages.append(page)
    assert len(workflows) == len(pages) == 4
    assert all(row["passed"] for row in workflows), workflows
    assert all(row["passed"] and len(row["executions"]) >= 2 for row in pages), pages
