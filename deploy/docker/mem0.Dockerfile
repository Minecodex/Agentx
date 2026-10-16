FROM python:3.12.12-slim

ADD --checksum=sha256:208fc22d1d60ecf3fa91230b30d0773381ea60457b83accac742410bacafa391 https://codeload.github.com/mem0ai/mem0/tar.gz/50bdaaea0c02744720ed374d88584fd01494eeb7 /tmp/mem0.tar.gz
RUN mkdir /app && tar -xzf /tmp/mem0.tar.gz --strip-components=2 -C /app mem0-50bdaaea0c02744720ed374d88584fd01494eeb7/server && rm /tmp/mem0.tar.gz
WORKDIR /app
RUN pip install --no-cache-dir -r requirements.txt 'mem0ai==2.0.15' 'psycopg[binary]>=3.2.8'
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

ENV PYTHONUNBUFFERED=1 MEM0_TELEMETRY=false
EXPOSE 8000
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8000"]
