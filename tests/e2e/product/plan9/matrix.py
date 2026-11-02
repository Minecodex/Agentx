"""The full twenty-scene matrix. Explicit subsets are smoke evidence, never full acceptance."""

import json
from pathlib import Path

from tests.e2e.product.next_batch_support import create_user, get
from tests.e2e.product.plan9 import agent_scenarios as agents
from tests.e2e.product.plan9 import approval_scenarios as approvals
from tests.e2e.product.plan9 import data_scenarios as data
from tests.e2e.product.plan9 import file_scenarios as files
from tests.e2e.product.plan9 import wait_scenarios as waits
from tests.e2e.product.plan9.long_report import long_report
from tests.e2e.product.plan9.scenarios import quotation
from tests.e2e.product.studio_builder_support import post
from tests.e2e.support import redact, run

SCENES = tuple(f"S{index:02}" for index in range(1, 21))


def reviewers(control, department, run_id):
    role = post(
        control,
        "/roles",
        {
            "name": f"Plan9 reviewer {run_id}",
            "code": f"p9_review_{run_id.replace('-', '_')}",
            "dataScope": "company",
            "permissions": ["approval:view", "approval:act"],
        },
    )
    return {
        actor: create_user(control, f"p9-{actor}-{run_id}", department, role["id"]) for actor in ("manager", "finance")
    }


def scenarios(control, profile, model, users, department, run_id, selected, request, installed):
    assert selected and set(selected).issubset(SCENES), f"Choose a nonempty explicit subset of {SCENES}"
    endpoint = ""
    if set(selected) & {"S04", "S05", "S17", "S18", "S19", "S20"}:
        endpoint = request.getfixturevalue("studio_provider").removesuffix("/v1")
        run(
            (
                "kubectl",
                "-n",
                installed["dependencies_namespace"],
                "set",
                "env",
                "deployment/echo-mcp",
                "AGENTX_FIXTURE_PLAN9=1",
            )
        )
        run(
            (
                "kubectl",
                "-n",
                installed["dependencies_namespace"],
                "rollout",
                "status",
                "deployment/echo-mcp",
                "--timeout=180s",
            ),
            timeout=200,
        )
    tool = (
        agents.crm_tool(control, department, users["manager"]["id"], endpoint, run_id)
        if set(selected) & {"S17", "S18", "S19"}
        else None
    )
    model = get(control, f"/models/aliases/{model['id']}")
    builders = {
        "S01": lambda: quotation(control, profile, run_id),
        "S02": lambda: data.procurement(control, profile, run_id),
        "S03": lambda: data.order_validation(control, profile, run_id),
        "S04": lambda: files.single_file(control, profile, run_id, endpoint),
        "S05": lambda: files.compare_files(control, profile, run_id, endpoint),
        "S06": lambda: agents.conversation(control, model, run_id),
        "S07": lambda: waits.plain_text(control, run_id),
        "S08": lambda: waits.delivery(control, run_id),
        "S09": lambda: waits.two_rounds(control, run_id),
        "S10": lambda: waits.conditional(control, profile, run_id),
        "S11": lambda: waits.parallel(control, run_id),
        "S12": lambda: waits.child(control, run_id),
        "S13": lambda: waits.loop_review(control, run_id),
        "S14": lambda: approvals.manager(control, run_id, users["manager"]["id"]),
        "S15": lambda: approvals.manager_finance(control, run_id, users["manager"]["id"], users["finance"]["id"]),
        "S16": lambda: approvals.threshold(control, run_id, users["manager"]["id"]),
        **{
            scene: (lambda scene=scene: agents.crm_application(control, model, profile, tool, endpoint, run_id, scene))
            for scene in ("S17", "S18", "S19")
        },
        "S20": lambda: long_report(control, profile, run_id, endpoint),
    }
    cases, preparation = [], []
    directory = Path(installed["artifact_dir"]) / "plan9" / "prepared-workflows"
    directory.mkdir(parents=True, exist_ok=True)
    for scene in selected:
        try:
            case = builders[scene]()
            definition = get(control, f"/workflows/{case['workflowId']}/versions/{case['versionId']}")
            (directory / f"{scene}.json").write_text(
                json.dumps({"scenario": case, "workflow": definition}, ensure_ascii=False, indent=2)
            )
            cases.append(case)
            preparation.append({"scene": scene, "prepared": True})
        except Exception as error:
            preparation.append({"scene": scene, "prepared": False, "error": redact(f"{type(error).__name__}: {error}")})
        (directory.parent / "scenario-preparation.json").write_text(
            json.dumps(preparation, ensure_ascii=False, indent=2)
        )
    assert cases, preparation
    return cases
