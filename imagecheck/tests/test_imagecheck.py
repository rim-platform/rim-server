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
from PIL import Image, ImageFont

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import imagecheck
from imagecheck import (
    CLIP_MEAN,
    CLIP_STD,
    ImageRejected,
    MAX_TEXT_BYTES,
    Model,
    Scorer,
    TEXT_MODEL,
    VitProcessor,
    clip_input,
    load_image,
    nsfw_index,
)

# The classifier's preprocessor_config.json at the pinned revision.
NSFW_PREPROCESSOR = {
    "do_convert_rgb": None,
    "do_normalize": True,
    "do_rescale": True,
    "do_resize": True,
    "image_mean": [0.5, 0.5, 0.5],
    "image_processor_type": "ViTFeatureExtractor",
    "image_std": [0.5, 0.5, 0.5],
    "resample": 2,
    "rescale_factor": 0.00392156862745098,
    "size": {"height": 224, "width": 224},
}


def png(image: Image.Image) -> bytes:
    out = io.BytesIO()
    image.save(out, "PNG")
    return out.getvalue()


def solid(rgb, size=(256, 256), mode="RGB") -> bytes:
    return png(Image.new(mode, size, rgb))


def stripes() -> Image.Image:
    """448x224: a red quarter on the left, a blue one on the right, green in the middle half."""
    image = Image.new("RGB", (448, 224), (0, 255, 0))
    image.paste((255, 0, 0), (0, 0, 112, 224))
    image.paste((0, 0, 255), (336, 0, 448, 224))
    return image


class LoadImageTest(unittest.TestCase):
    def test_grey_and_transparent_images_become_rgb(self):
        self.assertEqual(load_image(solid(128, mode="L")).mode, "RGB")
        self.assertEqual(load_image(solid((1, 2, 3, 0), mode="RGBA")).mode, "RGB")

    def test_refuses_images_over_1024_pixels(self):
        with self.assertRaises(ImageRejected) as caught:
            load_image(solid((0, 0, 0), size=(1025, 10)))
        self.assertEqual(caught.exception.status, 413)
        self.assertEqual(load_image(solid((0, 0, 0), size=(1024, 1024))).size, (1024, 1024))

    def test_refuses_files_over_1_mib(self):
        with self.assertRaises(ImageRejected) as caught:
            load_image(b"\x89PNG" + bytes(1024 * 1024))
        self.assertEqual(caught.exception.status, 413)

    def test_refuses_what_is_not_a_png(self):
        jpeg = io.BytesIO()
        Image.new("RGB", (8, 8)).save(jpeg, "JPEG")
        truncated = solid((1, 2, 3))[:60]
        for body in (b"", b"not an image", jpeg.getvalue(), truncated):
            with self.assertRaises(ImageRejected) as caught:
                load_image(body)
            self.assertEqual(caught.exception.status, 400)


class ClipInputTest(unittest.TestCase):
    def test_solid_colour_is_normalized_per_channel(self):
        pixels = clip_input(load_image(solid((200, 100, 50))))
        self.assertEqual(pixels.shape, (1, 3, 224, 224))
        self.assertEqual(pixels.dtype, np.float32)
        expected = (np.array([200, 100, 50]) / 255.0 - CLIP_MEAN) / CLIP_STD
        for channel in range(3):
            np.testing.assert_allclose(pixels[0, channel], expected[channel], atol=1e-5)

    def test_wide_image_keeps_the_centre(self):
        pixels = clip_input(stripes())
        self.assertEqual(pixels.shape, (1, 3, 224, 224))
        green = (np.array([0, 255, 0]) / 255.0 - CLIP_MEAN) / CLIP_STD
        for channel in range(3):
            np.testing.assert_allclose(pixels[0, channel, :, 4:-4], green[channel], atol=1e-5)

    def test_tall_small_image_is_scaled_up(self):
        pixels = clip_input(load_image(solid((10, 20, 30), size=(50, 120))))
        self.assertEqual(pixels.shape, (1, 3, 224, 224))


class VitProcessorTest(unittest.TestCase):
    def test_solid_colour_is_rescaled_and_normalized(self):
        pixels = VitProcessor(NSFW_PREPROCESSOR)(load_image(solid((255, 0, 51))))
        self.assertEqual(pixels.shape, (1, 3, 224, 224))
        self.assertEqual(pixels.dtype, np.float32)
        for channel, expected in enumerate((1.0, -1.0, -0.6)):
            np.testing.assert_allclose(pixels[0, channel], expected, atol=1e-5)

    def test_wide_image_is_squeezed_not_cropped(self):
        pixels = VitProcessor(NSFW_PREPROCESSOR)(stripes())
        self.assertEqual(pixels.shape, (1, 3, 224, 224))
        # Red stays in the left quarter and blue in the right one.
        np.testing.assert_allclose(pixels[0, 0, :, :50], 1.0, atol=1e-5)
        np.testing.assert_allclose(pixels[0, 2, :, -50:], 1.0, atol=1e-5)

    def test_flags_switch_steps_off(self):
        config = dict(NSFW_PREPROCESSOR, do_rescale=False, do_normalize=False)
        pixels = VitProcessor(config)(load_image(solid((255, 0, 51))))
        np.testing.assert_allclose(pixels[0, :, 0, 0], [255.0, 0.0, 51.0])

    def test_unsupported_configurations_are_refused(self):
        for config in (
            dict(NSFW_PREPROCESSOR, size={"shortest_edge": 224}),
            dict(NSFW_PREPROCESSOR, do_resize=False),
            dict(NSFW_PREPROCESSOR, do_center_crop=True),
        ):
            with self.assertRaises(ValueError):
                VitProcessor(config)


class ScoreTest(unittest.TestCase):
    def setUp(self):
        # Five orthogonal prompts: two sexual, one gore, two neutral; no violence prompt.
        self.scorer = Scorer(
            np.eye(5, dtype=np.float32),
            np.array(["sexual", "sexual", "gore", "neutral", "neutral"]),
            1.0,
        )

    def test_softmax_over_all_prompts_summed_per_category(self):
        scores = self.scorer.score(np.array([0, 0, 3.0, 0, 0]))
        # The embedding is normalized first, so its logits are [0, 0, 1, 0, 0].
        self.assertEqual(scores, {"gore": round(math.e / (math.e + 4), 4), "violence": 0.0})

    def test_sexual_prompts_take_probability_without_being_reported(self):
        scores = self.scorer.score(np.array([3.0, 0, 0, 0, 0]))
        self.assertEqual(scores, {"gore": round(1 / (math.e + 4), 4), "violence": 0.0})

    def test_logit_scale_sharpens(self):
        sharp = Scorer(self.scorer.embeds, self.scorer.labels, 100.0)
        self.assertEqual(sharp.score(np.array([0, 0, 1.0, 0, 0]))["gore"], 1.0)
        self.assertEqual(sharp.score(np.array([1.0, 0, 0, 0, 0]))["gore"], 0.0)

    def test_scores_are_probabilities(self):
        rng = np.random.default_rng(7)
        embeds = rng.normal(size=(12, 16)).astype(np.float32)
        embeds /= np.linalg.norm(embeds, axis=1, keepdims=True)
        labels = np.array(["sexual", "gore", "violence", "neutral"] * 3)
        scorer = Scorer(embeds, labels, 100.0)
        for _ in range(20):
            scores = scorer.score(rng.normal(size=16))
            self.assertEqual(list(scores), ["gore", "violence"])
            self.assertTrue(all(0.0 <= value <= 1.0 for value in scores.values()))
            self.assertLessEqual(sum(scores.values()), 1.0001)


class TextScoreTest(unittest.TestCase):
    def test_the_text_scorer_sums_only_the_sexual_rows(self):
        scorer = Scorer(
            np.eye(4, dtype=np.float32),
            np.array(["sexual", "sexual", "other", "other"]),
            1.0,
            ("sexual",),
        )
        scores = scorer.score(np.array([1.0, 0, 0, 0]))
        self.assertEqual(scores, {"sexual": round((math.e + 1) / (math.e + 3), 4)})


class NsfwIndexTest(unittest.TestCase):
    def test_finds_the_nsfw_label(self):
        self.assertEqual(nsfw_index({"id2label": {"0": "normal", "1": "nsfw"}}), 1)
        self.assertEqual(nsfw_index({"id2label": {"0": "nsfw", "1": "normal"}}), 0)

    def test_refuses_a_classifier_without_it(self):
        with self.assertRaises(ValueError):
            nsfw_index({"id2label": {"0": "cat", "1": "dog"}})


class StubSession:
    """An ONNX session that checks its one input and answers a fixed output."""

    def __init__(self, test, output, answer):
        self.test = test
        self.output = output
        self.answer = np.asarray([answer], dtype=np.float32)

    def run(self, outputs, inputs):
        self.test.assertEqual(outputs, [self.output])
        self.test.assertEqual(inputs["pixel_values"].shape, (1, 3, 224, 224))
        return [self.answer]


class ModelTest(unittest.TestCase):
    def model(self, logits, nsfw_label=1):
        clip = StubSession(self, "image_embeds", [0, 2.0, 0])
        scorer = Scorer(np.eye(3, dtype=np.float32), np.array(["sexual", "gore", "neutral"]), 1.0)
        nsfw = StubSession(self, "logits", logits)
        return Model(clip, scorer, nsfw, VitProcessor(NSFW_PREPROCESSOR), nsfw_label)

    def test_sexual_from_the_classifier_gore_and_violence_from_clip(self):
        scores = self.model([0.0, math.log(3)]).check(solid((9, 9, 9)))
        self.assertEqual(
            scores, {"sexual": 0.75, "gore": round(math.e / (math.e + 2), 4), "violence": 0.0}
        )

    def test_the_nsfw_label_picks_the_logit(self):
        scores = self.model([0.0, math.log(3)], nsfw_label=0).check(solid((9, 9, 9)))
        self.assertEqual(scores["sexual"], 0.25)

    def test_rejected_images_reach_no_model(self):
        with self.assertRaises(ImageRejected):
            self.model([0.0, 0.0]).check(b"not an image")


class TextModelTest(unittest.TestCase):
    def test_the_higher_of_the_two_polarities_is_reported(self):
        class Clip:
            def __init__(self):
                self.calls = 0

            def run(self, outputs, inputs):
                self.calls += 1
                return [np.array([[1.0, 0, 0] if self.calls == 1 else [0, 1.0, 0]])]

        model = Model(
            None,
            None,
            None,
            None,
            0,
            Clip(),
            Scorer(
                np.eye(3, dtype=np.float32),
                np.array(["sexual", "other", "other"]),
                1.0,
                ("sexual",),
            ),
            ImageFont.load_default(16),
        )
        scores = model.check_text("+--+\n|ab|\n+--+")
        self.assertEqual(scores, {"sexual": round(math.e / (math.e + 2), 4)})
        self.assertEqual(model.text_clip.calls, 2)


class StubModel:
    """Decodes like the real model, then answers fixed scores."""

    def __init__(self):
        self.calls = 0

    def check(self, png_bytes):
        load_image(png_bytes)
        self.calls += 1
        return {"sexual": 0.01, "gore": 0.02, "violence": 0.03}

    def check_text(self, text):
        self.texts = getattr(self, "texts", []) + [text]
        return {"sexual": 0.42}


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

    def check_text(self, body, content_type="text/plain"):
        return self.request("POST", "/v1/check-text", body, {"Content-Type": content_type})

    def test_check_text_answers_scores_and_model(self):
        self.server.model = StubModel()
        for content_type in ("text/plain", "text/plain; charset=utf-8"):
            status, content_type_out, body = self.check_text("⣿⠁ hi".encode(), content_type)
            self.assertEqual(status, 200, content_type)
            self.assertEqual(content_type_out, "application/json")
            self.assertEqual(json.loads(body), {"scores": {"sexual": 0.42}, "model": TEXT_MODEL})
        self.assertEqual(self.server.model.texts, ["⣿⠁ hi"] * 2)

    def test_check_text_other_type_is_415(self):
        self.server.model = StubModel()
        for content_type in ("application/json", "image/png", ""):
            status, _, _ = self.check_text(b"hello", content_type)
            self.assertEqual(status, 415, content_type)

    def test_check_text_without_length_is_411(self):
        self.server.model = StubModel()
        with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
            sock.sendall(
                b"POST /v1/check-text HTTP/1.1\r\nHost: x\r\nContent-Type: text/plain\r\n\r\n"
            )
            answer = sock.recv(4096)
        self.assertTrue(answer.startswith(b"HTTP/1.1 411 "), answer)

    def test_check_text_over_the_limit_is_413_before_the_body_is_read(self):
        self.server.model = StubModel()
        with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
            sock.sendall(
                b"POST /v1/check-text HTTP/1.1\r\nHost: x\r\nContent-Type: text/plain\r\n"
                b"Content-Length: 65537\r\n\r\n"
            )
            answer = sock.recv(4096)
        self.assertTrue(answer.startswith(b"HTTP/1.1 413 "), answer)
        self.assertFalse(hasattr(self.server.model, "texts"))

    def test_check_text_exactly_at_the_limit_is_accepted(self):
        self.server.model = StubModel()
        status, _, _ = self.check_text(b"a" * MAX_TEXT_BYTES)
        self.assertEqual(status, 200)

    def test_check_text_must_be_utf8_and_not_blank(self):
        self.server.model = StubModel()
        for body in (b"\xff\xfe\xfa", b"", b" \n\t  "):
            status, _, answer = self.check_text(body)
            self.assertEqual(status, 400, body)
            self.assertIn("error", json.loads(answer))
        self.assertFalse(hasattr(self.server.model, "texts"))

    def test_check_text_while_loading_is_503(self):
        status, _, _ = self.check_text(b"hello")
        self.assertEqual(status, 503)

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
                "model": "clip-vit-b32-q8@d15189d+falconsai-nsfw-q8@1ceb3c7",
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
