FROM python:3.12.12-slim

ARG PIP_INDEX_URL=https://pypi.org/simple

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    HF_HOME=/tmp/huggingface \
    HF_HUB_DISABLE_TELEMETRY=1 \
    HF_HUB_DISABLE_XET=1 \
    INFINITY_ANONYMOUS_USAGE_STATS=0 \
    OMP_NUM_THREADS=2 \
    MKL_NUM_THREADS=2

RUN pip install --no-cache-dir \
    'infinity-emb[server,torch]==0.0.77' \
    'transformers==4.57.1' \
    'torch==2.8.0'

# Keep the public model in the image so Pods never download weights at startup.
# The weight file is verified against the pinned Hugging Face LFS SHA-256.
RUN python - <<'PY'
import hashlib
from pathlib import Path
import requests
from huggingface_hub import snapshot_download

snapshot_download('BAAI/bge-small-zh-v1.5', revision='7999e1d3359715c523056ef9478215996d62a620', local_dir='/opt/model', allow_patterns=['*.json', '*.txt', '1_Pooling/*'])
digest = hashlib.sha256()
with requests.get('https://www.modelscope.cn/models/BAAI/bge-small-zh-v1.5/resolve/master/model.safetensors', stream=True, timeout=(15, 60)) as response:
    response.raise_for_status()
    with Path('/opt/model/model.safetensors').open('wb') as target:
        for chunk in response.iter_content(1024 * 1024):
            target.write(chunk)
            digest.update(chunk)
if digest.hexdigest() != '354763b9b1357bc9c44f62c6be2276321081ed2567773608c0d0785b61d5a026':
    raise RuntimeError('CPU embedding weight checksum does not match the pinned model')
PY
RUN chmod -R a+rX /opt/model

# Infinity 0.0.77 uses Typer 0.12, whose option flags require Click 8.1.
RUN pip install --no-cache-dir 'click==8.1.8'

ENV HF_HUB_OFFLINE=1 INFINITY_BETTERTRANSFORMER=false
USER 65532:65532
EXPOSE 7997
ENTRYPOINT ["infinity_emb", "v2"]
CMD ["--model-id", "/opt/model", "--served-model-name", "BAAI/bge-small-zh-v1.5", "--engine", "torch", "--device", "cpu", "--batch-size", "4", "--port", "7997", "--url-prefix", "/v1"]
