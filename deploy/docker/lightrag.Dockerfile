FROM ghcr.io/hkuds/lightrag@sha256:206579ab1a2bc1f2b5f19fe97c01cb801ff409e6ae29a15542f82208a285640a

ENV TIKTOKEN_CACHE_DIR=/opt/tiktoken-cache

RUN mkdir -p "$TIKTOKEN_CACHE_DIR" \
    && python -c "import tiktoken; tiktoken.encoding_for_model('gpt-4o-mini')" \
    && chmod -R a+rX "$TIKTOKEN_CACHE_DIR"

# Non-streaming calls must request JSON explicitly; some compatible endpoints
# default to SSE when stream is omitted. Preserve explicit streaming calls.
RUN python - <<'PY'
from pathlib import Path
path = Path("/app/lightrag/llm/openai.py")
source = path.read_text()
old = '    if stream is not None:\n        kwargs["stream"] = stream'
assert source.count(old) == 1
path.write_text(source.replace(old, '    kwargs["stream"] = False if stream is None else stream'))
PY
