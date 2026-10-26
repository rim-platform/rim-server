# =============================================================================
# rim-imagecheck — scores profile pictures for sexual content, gore and violence
# Multi-stage build: build (download the model, embed the prompts) → runtime
# =============================================================================

# ---------- Stage 1: Build (model files and prompt embeddings) ---------------
FROM python:3.12-slim AS build
WORKDIR /build

COPY requirements.txt requirements-build.txt ./
RUN pip install --no-cache-dir --no-deps -r requirements-build.txt

# CLIP ViT-B/32 as ONNX at a fixed revision; each file must match models.sha256. The int8
# vision model is what the service runs; the fp32 text model and the tokenizer only embed the
# prompts below and stay in this stage.
ARG MODEL_REPO=Xenova/clip-vit-base-patch32
ARG MODEL_REVISION=d15189d7028b43f1d3e65039190477f6af591c2a
COPY models.sha256 ./
RUN set -eu; \
    base="https://huggingface.co/$MODEL_REPO/resolve/$MODEL_REVISION"; \
    for path in onnx/vision_model_quantized.onnx onnx/text_model.onnx tokenizer.json; do \
        python -c 'import sys, urllib.request; urllib.request.urlretrieve(sys.argv[1], sys.argv[2])' \
            "$base/$path" "${path##*/}"; \
    done; \
    sha256sum -c models.sha256

COPY prompts.py build_prompts.py ./
RUN python build_prompts.py text_model.onnx tokenizer.json prompts.npz

# ---------- Stage 2: Runtime ------------------------------------------------
FROM python:3.12-slim AS runtime

# No bytecode files: the root filesystem is read-only when deployed. ONNX Runtime sends no
# usage telemetry.
ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    ORT_DISABLE_TELEMETRY=1

COPY requirements.txt /tmp/requirements.txt
RUN pip install --no-cache-dir --no-deps -r /tmp/requirements.txt && rm /tmp/requirements.txt

RUN groupadd --gid 1001 imagecheck && \
    useradd --uid 1001 --gid imagecheck --no-create-home --shell /bin/false imagecheck

WORKDIR /app
COPY --from=build /build/vision_model_quantized.onnx /build/prompts.npz ./
COPY imagecheck.py prompts.py ./

USER 1001

# Runtime configuration via environment variables:
#   PORT       — Listen port (default: 8090)
#   MODEL_DIR  — Directory of vision_model_quantized.onnx and prompts.npz (default: /app)
EXPOSE 8090

# Healthy once the model is loaded: /healthz answers 503 until then.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD ["python", "-c", "import os, urllib.request; urllib.request.urlopen('http://localhost:%s/healthz' % (os.environ.get('PORT') or 8090), timeout=4)"]

CMD ["python", "-u", "imagecheck.py"]
