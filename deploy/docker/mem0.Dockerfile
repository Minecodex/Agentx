# The structured OpenAI adapter and its dependencies already ship in this
# immutable Mem0 image. Enable it in the server's bundled-provider registry.
FROM agentx/mem0-server@sha256:6777dce26a2113c47ddeea3182cd0d10615a42169322bdbfd0def54a0969e5d5

RUN python - <<'PY'
from pathlib import Path
import importlib.util

assert importlib.util.find_spec("mem0.llms.openai_structured") is not None
path = Path("/app/main.py")
source = path.read_text()
previous = 'BUNDLED_LLM_PROVIDERS = ("openai", "anthropic", "gemini")'
assert source.count(previous) == 1
path.write_text(source.replace(
    previous,
    'BUNDLED_LLM_PROVIDERS = ("openai", "openai_structured", "anthropic", "gemini")',
))
PY
