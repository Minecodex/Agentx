"""Every selected scenario must be a valid, formally published workflow."""

import json

import pytest

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_all_selected_workflows_publish(plan9_suite):
    report = json.loads((plan9_suite["directory"] / "scenario-preparation.json").read_text())
    assert report and all(row["prepared"] for row in report), [row for row in report if not row["prepared"]]
