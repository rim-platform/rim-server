"""Scores profile pictures for sexual content, gore and violence, and text art for sexual content.

Two models run on every picture: a ViT image classifier trained to tell nsfw from normal pictures
(Falconsai/nsfw_image_detection) gives the sexual score, and CLIP ViT-B/32 compared with the
prompts in prompts.py gives gore and violence. Text art is drawn as a picture (textart.py) and
scored by CLIP's fp32 vision model against the prompts in text_prompts.py.

An HTTP service on PORT (default 8090):

    POST /v1/check   body: a PNG of at most 1 MiB and 1024x1024 pixels, Content-Type image/png
                     200 {"scores": {"sexual": s, "gore": g, "violence": v}, "model": MODEL}
                     400 unreadable image, 411 no Content-Length, 413 too large,
                     415 another type, 503 models not loaded yet
    POST /v1/check-text  body: UTF-8 text of at most 64 KiB, Content-Type text/plain
                     200 {"scores": {"sexual": s}, "model": TEXT_MODEL}
                     400 not UTF-8 or blank, 411 no Content-Length, 413 too large,
                     415 another type, 503 models not loaded yet
    GET  /healthz    200 "ok" once the models are loaded, 503 before

Each score is a probability from 0 to 1. What a score leads to is decided by the caller.

MODEL_DIR (default: this file's directory) holds the files named below.
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
from PIL import Image, ImageFont, ImageOps

import textart
from prompts import SCORED
from text_prompts import SCORED as TEXT_SCORED

# The models and revisions every answer carries.
MODEL = "clip-vit-b32-q8@d15189d+falconsai-nsfw-q8@1ceb3c7"
# The model every text answer carries.
TEXT_MODEL = "clip-vit-b32-fp32@d15189d+textart-v1"
# CLIP's fp32 vision model and the prompt embeddings of the text art check, and the font text
# art is drawn in.
CLIP_FP32_FILE = "vision_model.onnx"
TEXT_PROMPTS_FILE = "text_prompts.npz"
FONT_FILE = "DejaVuSansMono.ttf"
FONT_SIZE = 16
# Largest text request body, in bytes.
MAX_TEXT_BYTES = 65536
# CLIP's int8 vision model and the prompt embeddings build_prompts.py writes.
CLIP_FILE = "vision_model_quantized.onnx"
PROMPTS_FILE = "prompts.npz"
# The nsfw classifier's int8 model, its image processor settings and its model configuration.
NSFW_FILE = "nsfw_model_int8.onnx"
NSFW_PREPROCESSOR_FILE = "nsfw_preprocessor_config.json"
NSFW_CONFIG_FILE = "nsfw_config.json"
# Largest request body and image file, in bytes.
MAX_BYTES = 1024 * 1024
# Largest image width and height, in pixels.
MAX_SIDE = 1024
# The side of the square CLIP sees.
CLIP_SIDE = 224
CLIP_MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
CLIP_STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)

log = logging.getLogger("imagecheck")


class ImageRejected(Exception):
    """An image the service does not score, with the HTTP status that says why."""

    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status
        self.message = message


def load_image(png_bytes: bytes) -> Image.Image:
    """The PNG in `png_bytes` decoded to RGB.

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
        return image.convert("RGB")
    except ImageRejected:
        raise
    except Exception as err:
        raise ImageRejected(400, "the body is not a readable PNG image") from err


def clip_input(image: Image.Image) -> np.ndarray:
    """`image` as CLIP's input: float32 [1, 3, 224, 224], normalized.

    The shorter side is resized to 224 with bicubic resampling, the centre 224x224 is kept, and
    each channel is scaled to [0, 1] and normalized with CLIP's mean and standard deviation.
    """
    width, height = image.size
    size = (
        (CLIP_SIDE, max(CLIP_SIDE, int(CLIP_SIDE * height / width)))
        if width <= height
        else (max(CLIP_SIDE, int(CLIP_SIDE * width / height)), CLIP_SIDE)
    )
    image = image.resize(size, Image.Resampling.BICUBIC)
    left = (size[0] - CLIP_SIDE) // 2
    top = (size[1] - CLIP_SIDE) // 2
    image = image.crop((left, top, left + CLIP_SIDE, top + CLIP_SIDE))

    pixels = np.asarray(image, dtype=np.float32) / 255.0
    pixels = (pixels - CLIP_MEAN) / CLIP_STD
    return pixels.transpose(2, 0, 1)[np.newaxis].astype(np.float32)


class VitProcessor:
    """The input of a ViT image classifier, as its preprocessor_config.json describes it.

    The image is resized to the configured height and width with the configured resampling,
    rescaled by rescale_factor and normalized with image_mean and image_std, each step only when
    its do_ flag is on. A configuration this does not follow, such as a centre crop or a resize
    by shortest edge, raises ValueError.
    """

    def __init__(self, config: dict):
        size = config.get("size") or {}
        if not config.get("do_resize") or set(size) != {"height", "width"}:
            raise ValueError(f"unsupported resize in the image processor: {size}")
        if config.get("do_center_crop"):
            raise ValueError("unsupported centre crop in the image processor")
        self.size = (int(size["width"]), int(size["height"]))
        self.resample = Image.Resampling(int(config.get("resample", Image.Resampling.BILINEAR)))
        self.scale = float(config["rescale_factor"]) if config.get("do_rescale") else 1.0
        normalize = config.get("do_normalize")
        self.mean = np.array(config["image_mean"] if normalize else [0, 0, 0], dtype=np.float32)
        self.std = np.array(config["image_std"] if normalize else [1, 1, 1], dtype=np.float32)

    def __call__(self, image: Image.Image) -> np.ndarray:
        """`image` as the classifier's input: float32 [1, 3, height, width]."""
        image = image.resize(self.size, self.resample)
        pixels = np.asarray(image, dtype=np.float32) * self.scale
        pixels = (pixels - self.mean) / self.std
        return pixels.transpose(2, 0, 1)[np.newaxis].astype(np.float32)


def softmax(logits: np.ndarray) -> np.ndarray:
    """The softmax of a 1-dimensional array of logits."""
    probs = np.exp(logits - logits.max())
    return probs / probs.sum()


class Scorer:
    """Turns a CLIP image embedding into category scores against the prompt embeddings."""

    def __init__(
        self,
        embeds: np.ndarray,
        labels: np.ndarray,
        logit_scale: float,
        scored: tuple[str, ...] = SCORED,
    ):
        self.scored = scored
        self.embeds = embeds.astype(np.float32)
        self.labels = np.asarray(labels)
        self.logit_scale = float(logit_scale)

    def score(self, image_embed: np.ndarray) -> dict[str, float]:
        """The score of each scored category for `image_embed`, rounded to 4 places.

        The embedding is L2-normalized, its logits against every prompt are the logit scale times
        the dot products, a softmax runs over all prompts, and a category's score is the sum of
        its prompts' probabilities.
        """
        image = np.asarray(image_embed, dtype=np.float32).reshape(-1)
        image = image / np.linalg.norm(image)
        probs = softmax(self.logit_scale * (self.embeds @ image))
        return {
            category: round(float(probs[self.labels == category].sum()), 4)
            for category in self.scored
        }


def nsfw_index(config: dict) -> int:
    """The index of the `nsfw` label among the classifier's outputs, from its config.json."""
    for index, label in config.get("id2label", {}).items():
        if label == "nsfw":
            return int(index)
    raise ValueError("the classifier has no nsfw label")


class Model:
    """All models; one check runs at a time."""

    def __init__(
        self,
        clip,
        scorer: Scorer,
        nsfw,
        processor: VitProcessor,
        nsfw_label: int,
        text_clip=None,
        text_scorer: Scorer | None = None,
        font: ImageFont.FreeTypeFont | None = None,
    ):
        self.clip = clip
        self.scorer = scorer
        self.nsfw = nsfw
        self.processor = processor
        self.nsfw_label = nsfw_label
        self.text_clip = text_clip
        self.text_scorer = text_scorer
        self.font = font
        self.lock = threading.Lock()

    @classmethod
    def load(cls, model_dir: str) -> "Model":
        """Loads the model files from `model_dir`."""
        import onnxruntime as ort

        ort.disable_telemetry_events()
        options = ort.SessionOptions()
        options.intra_op_num_threads = 2
        options.inter_op_num_threads = 1
        # Idle worker threads sleep instead of spinning, so the session that ran last does not
        # take CPU from the one that runs next.
        options.add_session_config_entry("session.intra_op.allow_spinning", "0")

        def session(name):
            return ort.InferenceSession(
                os.path.join(model_dir, name),
                sess_options=options,
                providers=["CPUExecutionProvider"],
            )

        def config(name):
            with open(os.path.join(model_dir, name), encoding="utf-8") as file:
                return json.load(file)

        with np.load(os.path.join(model_dir, PROMPTS_FILE), allow_pickle=False) as data:
            scorer = Scorer(data["embeds"], data["labels"], float(data["logit_scale"]))
        with np.load(os.path.join(model_dir, TEXT_PROMPTS_FILE), allow_pickle=False) as data:
            text_scorer = Scorer(
                data["embeds"], data["labels"], float(data["logit_scale"]), TEXT_SCORED
            )
        return cls(
            session(CLIP_FILE),
            scorer,
            session(NSFW_FILE),
            VitProcessor(config(NSFW_PREPROCESSOR_FILE)),
            nsfw_index(config(NSFW_CONFIG_FILE)),
            session(CLIP_FP32_FILE),
            text_scorer,
            ImageFont.truetype(os.path.join(model_dir, FONT_FILE), FONT_SIZE),
        )

    def check(self, png_bytes: bytes) -> dict[str, float]:
        """The category scores of the PNG in `png_bytes`; raises ImageRejected like load_image.

        The sexual score is the classifier's nsfw probability; gore and violence come from CLIP.
        """
        image = load_image(png_bytes)
        clip_pixels = clip_input(image)
        nsfw_pixels = self.processor(image)
        # One after the other: on the container's two CPUs that is faster than both at once.
        with self.lock:
            (logits,) = self.nsfw.run(["logits"], {"pixel_values": nsfw_pixels})
            (embeds,) = self.clip.run(["image_embeds"], {"pixel_values": clip_pixels})
        sexual = float(softmax(logits[0])[self.nsfw_label])
        return {"sexual": round(sexual, 4), **self.scorer.score(embeds[0])}

    def check_text(self, text: str) -> dict[str, float]:
        """The sexual score of `text` drawn as a picture.

        The picture and its inversion go through the fp32 vision model; the score is the higher
        of the two.
        """
        image = textart.picture(text, self.font)
        scores = []
        with self.lock:
            for candidate in (image, ImageOps.invert(image)):
                (embeds,) = self.text_clip.run(
                    ["image_embeds"], {"pixel_values": clip_input(candidate)}
                )
                scores.append(self.text_scorer.score(embeds[0])["sexual"])
        return {"sexual": round(max(scores), 4)}


class Handler(BaseHTTPRequestHandler):
    """Answers /v1/check, /v1/check-text and /healthz; `self.server.model` is None while loading."""

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
        if self.path not in ("/v1/check", "/v1/check-text"):
            return self.send_json(404, {"error": "not found"})
        text = self.path == "/v1/check-text"
        limit = MAX_TEXT_BYTES if text else MAX_BYTES
        wanted = "text/plain" if text else "image/png"
        length = self.headers.get("Content-Length")
        if length is None or not length.isdigit():
            return self.send_json(411, {"error": "a Content-Length is required"})
        if int(length) > limit:
            return self.send_json(413, {"error": f"the body is larger than {limit} bytes"})
        media_type = (self.headers.get("Content-Type") or "").split(";")[0].strip().lower()
        if media_type != wanted:
            return self.send_json(415, {"error": f"the body must be {wanted}"})
        body = self.rfile.read(int(length))
        if text:
            return self.check_text(body)
        model = self.server.model
        if model is None:
            return self.send_json(503, {"error": "the models are still loading"})

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

    def check_text(self, body: bytes):
        """Answers /v1/check-text for the UTF-8 text in `body`."""
        try:
            text = body.decode("utf-8")
        except UnicodeDecodeError:
            return self.send_json(400, {"error": "the body is not UTF-8 text"})
        if not text.strip():
            return self.send_json(400, {"error": "the text is empty"})
        model = self.server.model
        if model is None:
            return self.send_json(503, {"error": "the models are still loading"})
        started = time.monotonic()
        scores = model.check_text(text)
        millis = (time.monotonic() - started) * 1000
        log.info("check-text sexual=%.4f chars=%d %.0f ms", scores["sexual"], len(text), millis)
        self.send_json(200, {"scores": scores, "model": TEXT_MODEL})

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
    """The HTTP server with the models it serves, None until they are loaded."""

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
            log.info("models %s loaded in %.1f s", MODEL, time.monotonic() - started)
        except Exception:
            log.exception("the models could not be loaded")
            os._exit(1)

    threading.Thread(target=load, daemon=True).start()
    log.info("listening on port %d", port)
    server.serve_forever()


if __name__ == "__main__":
    main()
