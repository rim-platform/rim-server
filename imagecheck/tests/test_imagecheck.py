"""Tests of the image check service that need no model: preprocessing, scoring and HTTP."""

import http.client
import io
import json
import math
import os
import socket
import sys
import threading
import unittest

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import imagecheck
from imagecheck import CLIP_MEAN, CLIP_STD, ImageRejected, Scorer, preprocess


def png(image: Image.Image) -> bytes:
    out = io.BytesIO()
    image.save(out, "PNG")
    return out.getvalue()


def solid(rgb, size=(256, 256), mode="RGB") -> bytes:
    return png(Image.new(mode, size, rgb))


class PreprocessTest(unittest.TestCase):
    def test_solid_colour_is_normalized_per_channel(self):
        pixels = preprocess(solid((200, 100, 50)))
        self.assertEqual(pixels.shape, (1, 3, 224, 224))
        self.assertEqual(pixels.dtype, np.float32)
        expected = (np.array([200, 100, 50]) / 255.0 - CLIP_MEAN) / CLIP_STD
        for channel in range(3):
            np.testing.assert_allclose(pixels[0, channel], expected[channel], atol=1e-5)

    def test_wide_image_keeps_the_centre(self):
        image = Image.new("RGB", (448, 224), (0, 255, 0))
        image.paste((255, 0, 0), (0, 0, 112, 224))
        image.paste((0, 0, 255), (336, 0, 448, 224))
        pixels = preprocess(png(image))
        self.assertEqual(pixels.shape, (1, 3, 224, 224))
        green = (np.array([0, 255, 0]) / 255.0 - CLIP_MEAN) / CLIP_STD
        for channel in range(3):
            np.testing.assert_allclose(pixels[0, channel, :, 4:-4], green[channel], atol=1e-5)

    def test_tall_small_image_is_scaled_up(self):
        pixels = preprocess(solid((10, 20, 30), size=(50, 120)))
        self.assertEqual(pixels.shape, (1, 3, 224, 224))

    def test_grey_and_transparent_images_become_rgb(self):
        self.assertEqual(preprocess(solid(128, mode="L")).shape, (1, 3, 224, 224))
        self.assertEqual(preprocess(solid((1, 2, 3, 0), mode="RGBA")).shape, (1, 3, 224, 224))

    def test_refuses_images_over_1024_pixels(self):
        with self.assertRaises(ImageRejected) as caught:
            preprocess(solid((0, 0, 0), size=(1025, 10)))
        self.assertEqual(caught.exception.status, 413)
        self.assertEqual(preprocess(solid((0, 0, 0), size=(1024, 1024))).shape, (1, 3, 224, 224))

    def test_refuses_files_over_1_mib(self):
        with self.assertRaises(ImageRejected) as caught:
            preprocess(b"\x89PNG" + bytes(1024 * 1024))
        self.assertEqual(caught.exception.status, 413)

    def test_refuses_what_is_not_a_png(self):
        jpeg = io.BytesIO()
        Image.new("RGB", (8, 8)).save(jpeg, "JPEG")
        truncated = solid((1, 2, 3))[:60]
        for body in (b"", b"not an image", jpeg.getvalue(), truncated):
            with self.assertRaises(ImageRejected) as caught:
                preprocess(body)
            self.assertEqual(caught.exception.status, 400)


class ScoreTest(unittest.TestCase):
    def setUp(self):
        # Five orthogonal prompts: two sexual, one gore, two neutral; no violence prompt.
        self.scorer = Scorer(
            np.eye(5, dtype=np.float32),
            np.array(["sexual", "sexual", "gore", "neutral", "neutral"]),
            1.0,
        )

    def test_softmax_over_all_prompts_summed_per_category(self):
        scores = self.scorer.score(np.array([3.0, 0, 0, 0, 0]))
        # The embedding is normalized first, so its logits are [1, 0, 0, 0, 0].
        total = math.e + 4
        self.assertEqual(
            scores,
            {
                "sexual": round((math.e + 1) / total, 4),
                "gore": round(1 / total, 4),
                "violence": 0.0,
            },
        )

    def test_logit_scale_sharpens(self):
        sharp = Scorer(self.scorer.embeds, self.scorer.labels, 100.0)
        scores = sharp.score(np.array([0, 0, 1.0, 0, 0]))
        self.assertEqual(scores["gore"], 1.0)
        self.assertEqual(scores["sexual"], 0.0)

    def test_scores_are_probabilities(self):
        rng = np.random.default_rng(7)
        embeds = rng.normal(size=(12, 16)).astype(np.float32)
        embeds /= np.linalg.norm(embeds, axis=1, keepdims=True)
        labels = np.array(["sexual", "gore", "violence", "neutral"] * 3)
        scorer = Scorer(embeds, labels, 100.0)
        for _ in range(20):
            scores = scorer.score(rng.normal(size=16))
            self.assertEqual(list(scores), ["sexual", "gore", "violence"])
            self.assertTrue(all(0.0 <= value <= 1.0 for value in scores.values()))
            self.assertLessEqual(sum(scores.values()), 1.0001)


class StubModel:
    """Preprocesses like the real model, then answers fixed scores."""

    def __init__(self):
        self.calls = 0

    def check(self, png_bytes):
        preprocess(png_bytes)
        self.calls += 1
        return {"sexual": 0.01, "gore": 0.02, "violence": 0.03}


class HttpTest(unittest.TestCase):
    def setUp(self):
        self.server = imagecheck.Server(("127.0.0.1", 0))
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(
            target=self.server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True
        )
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            conn.request(method, path, body=body, headers=headers or {})
            response = conn.getresponse()
            return response.status, response.getheader("Content-Type"), response.read()
        finally:
            conn.close()

    def check(self, body, content_type="image/png"):
        return self.request("POST", "/v1/check", body, {"Content-Type": content_type})

    def test_healthz_before_and_after_the_model_loads(self):
        status, _, body = self.request("GET", "/healthz")
        self.assertEqual(status, 503)
        self.server.model = StubModel()
        status, _, body = self.request("GET", "/healthz")
        self.assertEqual((status, body), (200, b"ok"))

    def test_check_answers_scores_and_model(self):
        self.server.model = StubModel()
        status, content_type, body = self.check(solid((9, 9, 9)), "image/png; charset=binary")
        self.assertEqual(status, 200)
        self.assertEqual(content_type, "application/json")
        self.assertEqual(
            json.loads(body),
            {
                "scores": {"sexual": 0.01, "gore": 0.02, "violence": 0.03},
                "model": "clip-vit-b32-q8@d15189d",
            },
        )

    def test_check_while_loading_is_503(self):
        status, _, _ = self.check(solid((9, 9, 9)))
        self.assertEqual(status, 503)

    def test_over_1_mib_is_413_before_the_body_is_read(self):
        self.server.model = StubModel()
        with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
            sock.sendall(
                b"POST /v1/check HTTP/1.1\r\nHost: x\r\nContent-Type: image/png\r\n"
                b"Content-Length: 1048577\r\n\r\n"
            )
            answer = sock.recv(4096)
        self.assertTrue(answer.startswith(b"HTTP/1.1 413 "), answer)
        self.assertEqual(self.server.model.calls, 0)

    def test_exactly_1_mib_is_read(self):
        self.server.model = StubModel()
        status, _, _ = self.check(b"x" * (1024 * 1024))
        self.assertEqual(status, 400)

    def test_other_type_is_415(self):
        self.server.model = StubModel()
        for content_type in ("image/jpeg", "application/octet-stream", ""):
            status, _, _ = self.check(solid((9, 9, 9)), content_type)
            self.assertEqual(status, 415, content_type)
        self.assertEqual(self.server.model.calls, 0)

    def test_unreadable_image_is_400(self):
        self.server.model = StubModel()
        status, _, body = self.check(b"\x89PNG\r\n\x1a\n garbage")
        self.assertEqual(status, 400)
        self.assertIn("error", json.loads(body))

    def test_oversized_image_is_413(self):
        self.server.model = StubModel()
        status, _, _ = self.check(solid((0, 0, 0), size=(2000, 10)))
        self.assertEqual(status, 413)

    def test_no_content_length_is_411(self):
        self.server.model = StubModel()
        with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
            sock.sendall(b"POST /v1/check HTTP/1.1\r\nHost: x\r\nContent-Type: image/png\r\n\r\n")
            answer = sock.recv(4096)
        self.assertTrue(answer.startswith(b"HTTP/1.1 411 "), answer)

    def test_everything_else_is_404(self):
        self.server.model = StubModel()
        for method, path in (
            ("GET", "/"),
            ("GET", "/v1/check"),
            ("POST", "/healthz"),
            ("POST", "/v1/check/"),
            ("PUT", "/v1/check"),
            ("DELETE", "/healthz"),
        ):
            status, _, _ = self.request(method, path, b"" if method != "GET" else None)
            self.assertEqual(status, 404, f"{method} {path}")


if __name__ == "__main__":
    unittest.main()
