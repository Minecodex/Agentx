"""No model request: prove independent-client configuration and daily-cache isolation first."""

import json
from pathlib import Path

from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.support import ROOT


def test_opencode_private_configuration(run_id):
    directory = ROOT / ".local/artifacts/e2e" / run_id / "opencode-preflight"
    directory.mkdir(parents=True, exist_ok=True)
    with isolated_opencode("https://127.0.0.1:1/v1", "synthetic-relay-credential") as client:
        root = client.root
        paths = dict(line.split(maxsplit=1) for line in client.execute(["debug", "paths"], timeout=60).splitlines())
        for key in ("home", "config", "data", "cache", "state"):
            assert Path(paths[key]).is_relative_to(root), (key, paths)
        config = json.loads(client.execute(["debug", "config"], timeout=60))
        assert config["model"] == config["small_model"] == "apimart/glm-5.3-flash"
        assert config["permission"] == {"*": "deny", "application_*": "allow"}
        assert config["plugin"] == []
        assert config["provider"]["apimart"]["models"]["glm-5.3-flash"]["options"]["reasoningEffort"] == "low"
        assert not config.get("mcp")
        (directory / "isolation.json").write_text(
            json.dumps(
                {
                    "version": "1.1.56",
                    "privatePaths": True,
                    "model": config["model"],
                    "permission": config["permission"],
                    "plugins": [],
                    "status": "passed",
                },
                indent=2,
            )
        )
    assert not root.exists(), "Private credentials and client caches must be removed"
