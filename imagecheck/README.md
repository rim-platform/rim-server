# rim-imagecheck

A small HTTP service that scores a profile picture for sexual content, gore and violence before
rim-server stores it, and text art for sexual content. It runs next to rim-server as the
`imagecheck` service of `deploy/compose.yml`, on the internal network only, and is published with every release as
`ghcr.io/rim-platform/rim-imagecheck`.

Two models run on every picture, both as int8 ONNX with ONNX Runtime on the CPU:

- **Sexual content**: [Falconsai/nsfw_image_detection](https://huggingface.co/Falconsai/nsfw_image_detection),
  a ViT-base classifier trained to tell `nsfw` from `normal` pictures, as converted in
  [`onnx-community/nsfw_image_detection-ONNX`](https://huggingface.co/onnx-community/nsfw_image_detection-ONNX)
  at revision `1ceb3c7fe1e9f3f2507e6df577437f23a9149fd5` (`onnx/model_int8.onnx`). The score is
  the softmax probability of `nsfw`. The picture is prepared as the model's
  `preprocessor_config.json` says: resized to 224×224, bilinear, normalized with mean and
  standard deviation 0.5.
- **Gore and violence**: [CLIP ViT-B/32](https://github.com/openai/CLIP)
  ([`Xenova/clip-vit-base-patch32`](https://huggingface.co/Xenova/clip-vit-base-patch32) at
  revision `d15189d7028b43f1d3e65039190477f6af591c2a`) compares the picture with the text
  prompts in [`prompts.py`](prompts.py). A softmax over all prompts gives each a probability, and
  a category's score is the sum over its prompts. The neutral prompts (people, pets, drawings,
  logos, textures, pages of text…) give ordinary pictures somewhere to go. CLIP's sexual prompts
  stay in the softmax without being reported: they take the probability of nude pictures that
  would otherwise go to gore or violence.

Text art is scored by CLIP alone, see [Text art](#text-art).

Nothing leaves the container: the model files are part of the image, and ONNX Runtime's usage
telemetry is switched off.

## API

`POST /v1/check` with `Content-Type: image/png` and a PNG of at most 1 MiB and 1024×1024 pixels
(rim-server sends the 256×256 PNG it stores):

```json
{"scores": {"sexual": 0.0001, "gore": 0.005, "violence": 0.0065}, "model": "clip-vit-b32-q8@d15189d+falconsai-nsfw-q8@1ceb3c7"}
```

Each score is between 0 and 1, rounded to four places; `model` names both models and their
revisions. Other answers carry `{"error": "…"}`:

| Status | When |
|---|---|
| 400 | the body is not a readable PNG |
| 411 | no `Content-Length` |
| 413 | `Content-Length` over 1 MiB (refused before the body is read), or an image over 1024×1024 |
| 415 | another `Content-Type` |
| 503 | the models are still loading |
| 404 | any other path or method |

`POST /v1/check-text` with `Content-Type: text/plain` (parameters such as `; charset=utf-8` are
allowed) and a message's text, UTF-8, of at most 64 KiB (65,536 bytes):

```json
{"scores": {"sexual": 0.0326}, "model": "clip-vit-b32-fp32@d15189d+textart-v1"}
```

The score is between 0 and 1, rounded to four places. Errors carry `{"error": "…"}`:

| Status | When |
|---|---|
| 400 | the body is not UTF-8, or is empty or only whitespace |
| 411 | no `Content-Length` |
| 413 | `Content-Length` over 65,536 bytes (refused before the body is read) |
| 415 | a `Content-Type` other than `text/plain` |
| 503 | the models are still loading |
| 404 | any other path or method |

`GET /healthz` answers `200 ok` once the models are loaded and `503` before; the image's
`HEALTHCHECK` probes it. Each check logs one line with its scores and its duration (a text check
also its number of characters); image data and text are never logged.

Settings: `PORT` (default 8090) and `MODEL_DIR` (default the script's directory, `/app` in the
image).

## Text art

rim-server sends `/v1/check-text` only messages that look like text art (braille, block or
box-drawing pictures, ASCII art), and decides from the score it gets back. The text is drawn as
a picture the way a client shows it: a text of which more than half of the non-whitespace
characters are braille is decoded dot for dot (every character a 2×4 cell, raised dots black on
white) and blurred; any other text is set in DejaVu Sans Mono and blurred more. The result is
scaled to fit 256×256, centred on a white square, and scored in both polarities, the picture and
its inversion; the higher score counts. At most 200 lines of 400 characters are drawn.

CLIP ViT-B/32 compares the picture with the prompts in [`text_prompts.py`](text_prompts.py),
written for drawings, line art, dotted and edge renderings and text art; a softmax over all
prompts gives the score, the sum over the `sexual` ones. The text path runs CLIP's **fp32**
vision model, not the int8 one the profile picture check uses: on the same pixels the int8
model's embedding is only about 0.92 cosine-similar to the fp32 one, and a known sexual sample
dropped from 0.83 to 0.57.

Calibration, measured with the prompts above on 152 harmless samples and a few sexual ones (the
sexual samples were braille art):

| Samples | sexual score |
|---|---|
| 152 harmless: braille art in three styles from photos, portraits, drawings (anime too) and icons; banners, tables, code, logs, emoticons | median 0.04, highest 0.37 |
| a bikini photo as braille art (borderline) | 0.57 |
| The Creation of Adam as braille art | at most 0.16 |
| two sexual braille samples | 0.83 and 0.77 |

The sexual side rests on very few samples, so the threshold should be reviewed as reports come in.

## Scores and actions

The service only scores; rim-server decides, from the highest of the three scores:

| Highest score | rim-server | Setting (default) |
|---|---|---|
| at or above the block threshold | refuses the picture with 422, stores nothing | `RIM_AVATAR_CHECK_BLOCK` (0.85) |
| at or above the report threshold | stores it and files an abuse report for the operators | `RIM_AVATAR_CHECK_REPORT` (0.60) |
| below both | stores it | |

With `RIM_AVATAR_CHECK_URL` unset rim-server checks nothing; with it set and the service down,
picture uploads fail with 503 and everything else keeps working.

## Calibration on benign images

Each image was cropped to a square, scaled to a 256×256 PNG and scored by the service as built
here. No sexual, gory or violent image was downloaded or generated, so these numbers say how
much ordinary pictures score, not how well hits are found. Operators tune the thresholds on
their own traffic.

| Image (scikit-image sample or generated) | sexual | gore | violence |
|---|---|---|---|
| astronaut | 0.0002 | 0.0513 | 0.0048 |
| camera | 0.0001 | 0.0041 | 0.0176 |
| coffee | 0.0001 | 0.0257 | 0.0153 |
| chelsea (cat) | 0.0001 | 0.0050 | 0.0065 |
| rocket | 0.0001 | 0.0085 | 0.0039 |
| hubble_deep_field | 0.0006 | 0.0038 | 0.0020 |
| coins (greyscale) | 0.0002 | 0.0000 | 0.0000 |
| horse (silhouette) | 0.0002 | 0.0177 | 0.0030 |
| moon | 0.0006 | 0.0048 | 0.0046 |
| page (greyscale scan of text) | 0.0002 | 0.0102 | 0.0016 |
| text (greyscale photo of text) | 0.0013 | 0.0316 | 0.0074 |
| immunohistochemistry (microscope) | 0.0002 | 0.0064 | 0.0024 |
| clock | 0.0153 | 0.0014 | 0.0022 |
| logo | 0.0003 | 0.0741 | 0.0128 |
| grass | 0.0026 | 0.0050 | 0.0011 |
| gravel | 0.0010 | 0.0056 | 0.0012 |
| brick | 0.0004 | 0.0017 | 0.0004 |
| colorwheel | 0.0003 | 0.0156 | 0.0030 |
| retina | 0.0002 | 0.0054 | 0.0056 |
| solid white | 0.0095 | 0.0123 | 0.0120 |
| solid black | 0.0161 | 0.0813 | 0.0402 |
| solid grey | 0.0058 | 0.0133 | 0.0182 |
| solid green | 0.0014 | 0.0062 | 0.0046 |
| solid blue | 0.0018 | 0.0209 | 0.0093 |
| skin tone (solid) | 0.0077 | 0.0088 | 0.0021 |
| pure red | 0.0029 | 0.0116 | 0.0018 |
| dark red | 0.0017 | 0.0261 | 0.0069 |
| gradient black to white | 0.0028 | 0.0061 | 0.0044 |
| gradient red to blue | 0.0009 | 0.0385 | 0.0251 |
| radial red gradient | 0.0052 | 0.0005 | 0.0001 |
| random noise | 0.0004 | 0.0773 | 0.0033 |
| rendered text, black on white | 0.0012 | 0.0066 | 0.0034 |
| rendered text "BLOOD RED", white on red | 0.0007 | **0.6485** | 0.0040 |

Every image but one stays below 0.10 in every category. Text on a red background lands on "a
pool of blood" and reaches the report threshold, not the block one. When CLIP's prompts also
scored sexual content, greyscale textures and pages of text reached 0.89 (coins 0.83,
immunohistochemistry 0.89, page 0.66), nearest "a photo of exposed breasts". The classifier
scores them at 0.0002, and the neutral prompts for textures, microscope images and pages of
text keep them away from CLIP's other categories.

## Resources

On a 20-CPU host, with the limits `deploy/compose.yml` sets (1536 MiB, 2 CPUs):

- image: 811 MB (Python 3.12 slim, ONNX Runtime, NumPy, Pillow, the 89 MB int8 and 352 MB fp32
  CLIP vision models, the 87 MB classifier and the font);
- memory: about 825 MiB resident with all models loaded, after a few checks of each kind, and
  about 845 MiB after a burst of 8 text checks at a time (the limit was 1 GB before the fp32
  model and is 1536 MiB now);
- latency: about 105 ms per profile picture check and about 100 to 115 ms per text check (the
  service's own log line, a 2.1 to 2.7 thousand character text), one at a time; checks run one
  at a time on 2 threads;
- start: all models load in under a second.

## Licences

- **The nsfw classifier**: Falconsai/nsfw_image_detection is released under the Apache-2.0
  licence (`license: apache-2.0` on its model card). The ONNX conversion,
  `onnx-community/nsfw_image_detection-ONNX`, declares no licence of its own and names it as its
  base model.
- **CLIP**: OpenAI releases CLIP's code and weights under the MIT licence
  ([LICENSE](https://github.com/openai/CLIP/blob/main/LICENSE)). Neither Hugging Face model card,
  `openai/clip-vit-base-patch32` nor its ONNX conversion `Xenova/clip-vit-base-patch32`, declares
  a licence of its own. OpenAI's model card calls deployed use of CLIP out of scope without
  task-specific testing; the calibration above is the start of that testing for this prompt set,
  and operators should extend it on their own traffic before relying on the block threshold.

- **The font**: text art is drawn in DejaVu Sans Mono from Debian's `fonts-dejavu-core`. The
  DejaVu fonts are derived from Bitstream Vera and are free to use, copy and embed under the
  Bitstream Vera and DejaVu licence (permissive, no copyleft).

The service's own code is under the repository's Apache-2.0 licence.

## Building and testing

```sh
docker build -t rim-imagecheck:local imagecheck
docker run --rm --read-only --tmpfs /tmp -p 8090:8090 rim-imagecheck:local
curl -s --data-binary @picture.png -H 'Content-Type: image/png' http://localhost:8090/v1/check
curl -s --data-binary @art.txt -H 'Content-Type: text/plain' http://localhost:8090/v1/check-text

pip install --no-deps -r imagecheck/requirements.txt
python -m unittest discover -s imagecheck/tests
```

The build downloads seven model files, checks them against [`models.sha256`](models.sha256) and
runs [`build_prompts.py`](build_prompts.py) once per prompt set. From CLIP: the int8 and fp32
vision models, plus the fp32 text model and the tokenizer, which only embed the prompts at build
time. From the classifier: the int8 model, its `preprocessor_config.json` and its `config.json` (for the label order), saved
with an `nsfw_` prefix. The tests need no model.
