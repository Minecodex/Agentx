from pathlib import Path

from tests.e2e.product import test_canvas_performance as performance


def test_isolated_benchmark_uses_the_exposed_web_entrypoint(monkeypatch, tmp_path):
    calls = []

    class Client:
        def __init__(self, **options):
            calls.append(("client", options["base_url"]))

        def __enter__(self):
            return self

        def __exit__(self, *_):
            return None

    monkeypatch.setattr(performance.httpx, "Client", Client)
    monkeypatch.setattr(performance, "_access_token", lambda _: calls.append(("bootstrap",)))
    monkeypatch.setattr(performance.subprocess, "run", lambda *args, **kwargs: calls.append(("build",)))
    monkeypatch.setattr(
        performance,
        "run_playwright",
        lambda root, suite, tests, environment: calls.append(
            ("benchmark", Path(root), suite, tests, environment["AGENTX_E2E_BASE_URL"])
        ),
    )
    performance.test_workflow_canvas_performance(
        {"root": str(tmp_path), "run_id": "performance-proof"},
        {"web": "http://127.0.0.1:18080", "runtime": "http://127.0.0.1:18081"},
    )
    assert calls[0] == ("client", "http://127.0.0.1:18080")
    assert calls[1] == ("bootstrap",)
    assert calls[-1] == (
        "benchmark",
        tmp_path,
        "workflow-performance",
        ("tests/workflow-canvas-performance.spec.ts", "--workers=1"),
        "http://127.0.0.1:18080",
    )
