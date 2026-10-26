"""Scores profile pictures for sexual content, gore and violence with CLIP ViT-B/32.

An HTTP service on PORT (default 8090):

    POST /v1/check   body: a PNG of at most 1 MiB and 1024x1024 pixels, Content-Type image/png
                     200 {"scores": {"sexual": s, "gore": g, "violence": v}, "model": MODEL}
                     400 unreadable image, 411 no Content-Length, 413 too large,
                     415 another type, 503 model not loaded yet
    GET  /healthz    200 "ok" once the model is loaded, 503 before

Each score is the probability CLIP gives that category's prompts among all prompts in
prompts.py, from 0 to 1. What a score leads to is decided by the caller.

MODEL_DIR (default: this file's directory) holds vision_model_quantized.onnx and prompts.npz.
"""

import io
import json
import logging
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import numpy as np
from PIL import Image

from prompts import SCORED

# The model name and revision every answer carries.
MODEL = "clip-vit-b32-q8@d15189d"
# Largest request body and image file, in bytes.
MAX_BYTES = 1024 * 1024
# Largest image width and height, in pixels.
MAX_SIDE = 1024
# The side of the square the model sees.
INPUT_SIDE = 224
CLIP_MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
CLIP_STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)

log = logging.getLogger("imagecheck")


class ImageRejected(Exception):
    """An image the service does not score, with the HTTP status that says why."""

    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status
        self.message = message


def preprocess(png_bytes: bytes) -> np.ndarray:
    """The PNG in `png_bytes` as CLIP's input: float32 [1, 3, 224, 224], normalized.

    The shorter side is resized to 224 with bicubic resampling, the centre 224x224 is kept, and
    each channel is scaled to [0, 1] and normalized with CLIP's mean and standard deviation.

    Raises ImageRejected (413) for a file over MAX_BYTES or an image wider or taller than
    MAX_SIDE, and (400) for anything that is not a readable PNG.
    """
    if len(png_bytes) > MAX_BYTES:
        raise ImageRejected(413, f"the image is larger than {MAX_BYTES} bytes")
    try:
        image = Image.open(io.BytesIO(png_bytes), formats=["PNG"])
        width, height = image.size
        if width > MAX_SIDE or height > MAX_SIDE:
            raise ImageRejected(413, f"the image is larger than {MAX_SIDE}x{MAX_SIDE} pixels")
        image = image.convert("RGB")
    except ImageRejected:
        raise
    except Exception as err:
        raise ImageRejected(400, "the body is not a readable PNG image") from err

    short = min(width, height)
    size = (
        (INPUT_SIDE, max(INPUT_SIDE, int(INPUT_SIDE * height / width)))
        if width == short
        else (max(INPUT_SIDE, int(INPUT_SIDE * width / height)), INPUT_SIDE)
    )
    image = image.resize(size, Image.Resampling.BICUBIC)
    left = (size[0] - INPUT_SIDE) // 2
    top = (size[1] - INPUT_SIDE) // 2
    image = image.crop((left, top, left + INPUT_SIDE, top + INPUT_SIDE))

    pixels = np.asarray(image, dtype=np.float32) / 255.0
    pixels = (pixels - CLIP_MEAN) / CLIP_STD
    return pixels.transpose(2, 0, 1)[np.newaxis].astype(np.float32)


class Scorer:
    """Turns an image embedding into category scores against the prompt embeddings."""

    def __init__(self, embeds: np.ndarray, labels: np.ndarray, logit_scale: float):
        self.embeds = embeds.astype(np.float32)
        self.labels = np.asarray(labels)
        self.logit_scale = float(logit_scale)

    def score(self, image_embed: np.ndarray) -> dict[str, float]:
        """The score of each category in SCORED for `image_embed`, rounded to 4 places.

        The embedding is L2-normalized, its logits against every prompt are the logit scale times
        the dot products, a softmax runs over all prompts, and a category's score is the sum of
        its prompts' probabilities.
        """
        image = np.asarray(image_embed, dtype=np.float32).reshape(-1)
        image = image / np.linalg.norm(image)
        logits = self.logit_scale * (self.embeds @ image)
        probs = np.exp(logits - logits.max())
        probs /= probs.sum()
        return {
            category: round(float(probs[self.labels == category].sum()), 4) for category in SCORED
        }


class Model:
    """The vision model and the scorer; one inference runs at a time."""

    def __init__(self, session, scorer: Scorer):
        self.session = session
        self.scorer = scorer
        self.lock = threading.Lock()

    @classmethod
    def load(cls, model_dir: str) -> "Model":
        """Loads vision_model_quantized.onnx and prompts.npz from `model_dir`."""
        import onnxruntime as ort

        ort.disable_telemetry_events()
        options = ort.SessionOptions()
        options.intra_op_num_threads = 2
        options.inter_op_num_threads = 1
        session = ort.InferenceSession(
            os.path.join(model_dir, "vision_model_quantized.onnx"),
            sess_options=options,
            providers=["CPUExecutionProvider"],
        )
        with np.load(os.path.join(model_dir, "prompts.npz"), allow_pickle=False) as data:
            scorer = Scorer(data["embeds"], data["labels"], float(data["logit_scale"]))
        return cls(session, scorer)

    def check(self, png_bytes: bytes) -> dict[str, float]:
        """The category scores of the PNG in `png_bytes`; raises ImageRejected like preprocess."""
        pixels = preprocess(png_bytes)
        with self.lock:
            (embeds,) = self.session.run(["image_embeds"], {"pixel_values": pixels})
        return self.scorer.score(embeds[0])


class Handler(BaseHTTPRequestHandler):
    """Answers /v1/check and /healthz with the model on `self.server.model` (None while loading)."""

    server_version = "imagecheck"
    sys_version = ""
    protocol_version = "HTTP/1.1"
    # Seconds a connection may sit idle or send its body slowly.
    timeout = 10

    def do_GET(self):
        if self.path != "/healthz":
            return self.send_json(404, {"error": "not found"})
        if self.server.model is None:
            return self.send_text(503, "loading")
        self.send_text(200, "ok")

    def do_POST(self):
        if self.path != "/v1/check":
            return self.send_json(404, {"error": "not found"})
        length = self.headers.get("Content-Length")
        if length is None or not length.isdigit():
            return self.send_json(411, {"error": "a Content-Length is required"})
        if int(length) > MAX_BYTES:
            return self.send_json(413, {"error": f"the body is larger than {MAX_BYTES} bytes"})
        media_type = (self.headers.get("Content-Type") or "").split(";")[0].strip().lower()
        if media_type != "image/png":
            return self.send_json(415, {"error": "the body must be image/png"})
        body = self.rfile.read(int(length))
        model = self.server.model
        if model is None:
            return self.send_json(503, {"error": "the model is still loading"})

        started = time.monotonic()
        try:
            scores = model.check(body)
        except ImageRejected as err:
            return self.send_json(err.status, {"error": err.message})
        millis = (time.monotonic() - started) * 1000
        log.info(
            "check %s %.0f ms",
            " ".join(f"{category}={value:.4f}" for category, value in scores.items()),
            millis,
        )
        self.send_json(200, {"scores": scores, "model": MODEL})

    def do_PUT(self):
        self.send_json(404, {"error": "not found"})

    do_DELETE = do_PATCH = do_HEAD = do_OPTIONS = do_PUT

    def send_json(self, status: int, body: dict):
        self.send_body(status, "application/json", json.dumps(body).encode())

    def send_text(self, status: int, text: str):
        self.send_body(status, "text/plain; charset=utf-8", text.encode())

    def send_body(self, status: int, content_type: str, data: bytes):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        if status >= 400:
            # A request body may be left unread, so the connection is not reused.
            self.send_header("Connection", "close")
            self.close_connection = True
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def log_message(self, format, *args):
        """Request lines are not logged; each check logs its scores instead."""


class Server(ThreadingHTTPServer):
    """The HTTP server with the model it serves, None until it is loaded."""

    daemon_threads = True

    def __init__(self, address, model=None):
        super().__init__(address, Handler)
        self.model = model


def main() -> None:
    logging.basicConfig(
        level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s", stream=sys.stdout
    )
    port = int(os.environ.get("PORT") or 8090)
    model_dir = os.environ.get("MODEL_DIR") or os.path.dirname(os.path.abspath(__file__))
    server = Server(("0.0.0.0", port))

    def load():
        try:
            started = time.monotonic()
            server.model = Model.load(model_dir)
            log.info("model %s loaded in %.1f s", MODEL, time.monotonic() - started)
        except Exception:
            log.exception("the model could not be loaded")
            os._exit(1)

    threading.Thread(target=load, daemon=True).start()
    log.info("listening on port %d", port)
    server.serve_forever()


if __name__ == "__main__":
    main()
