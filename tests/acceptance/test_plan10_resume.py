"""A kept-environment retry must reject unowned or changed candidates before I/O."""

import json
from types import SimpleNamespace

import pytest

from tests.e2e.product.plan10 import resume


@pytest.mark.parametrize("violation", ["production_namespace", "changed_source", "unrelated_gate", "upgrade_namespace"])
def test_resume_rejects_unsafe_reuse_before_cluster_access(tmp_path, monkeypatch, violation):
    identity = {"identity": {"sourceTreeSha256": "a" * 64}, "images": []}
    (tmp_path / "candidate-identity.json").write_text(json.dumps(identity))
    context = {
        "run_id": "plan10-owned",
        "artifact_dir": str(tmp_path),
        "root": str(tmp_path),
        **{
            f"{plane}_namespace": f"agentx-e2e-{plane}-plan10-owned" for plane in ("control", "runtime", "dependencies")
        },
    }
    monkeypatch.setattr(resume, "source_tree_sha256", lambda _: "b" * 64 if violation == "changed_source" else "a" * 64)
    if violation in {"production_namespace", "upgrade_namespace"}:
        context["control_namespace"] = "agentx-prod-control"
    filename = "test_unrelated.py" if violation == "unrelated_gate" else "test_06_real_pages.py"
    accesses = []

    def cluster(*args, **kwargs):
        accesses.append(args)
        raise AssertionError("Unsafe reuse reached the cluster")

    monkeypatch.setattr(resume, "run", cluster)
    with pytest.raises(AssertionError):
        resume.retain_candidate(
            context,
            [SimpleNamespace(path=tmp_path / "tests/e2e/product/plan10" / filename)],
            upgrade=violation == "upgrade_namespace",
        )
    assert accesses == []


@pytest.mark.parametrize(
    ("review_hash", "review_passed", "expected"),
    [("current", False, False), ("older", False, True), ("current", True, True)],
)
def test_resume_never_skips_a_page_with_failed_current_visual_review(tmp_path, review_hash, review_passed, expected):
    (tmp_path / "expense-complete.json").write_text(
        json.dumps({"passed": True, "publication": {"htmlHash": "current"}})
    )
    (tmp_path / "expense-visual-review.json").write_text(json.dumps({"htmlHash": review_hash, "passed": review_passed}))
    assert resume.completed_page(tmp_path, "expense") is expected


def test_resume_needs_actual_business_completion(tmp_path):
    assert not resume.completed_page(tmp_path, "expense")
