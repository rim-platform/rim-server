# rim-imagecheck

A small HTTP service that scores a profile picture for sexual content, gore and violence before
rim-server stores it. It runs next to rim-server as the `imagecheck` service of
`deploy/compose.yml`, on the internal network only, and is published with every release as
`ghcr.io/rim-platform/rim-imagecheck`.

The model is [CLIP ViT-B/32](https://github.com/openai/CLIP) as int8 ONNX
([`Xenova/clip-vit-base-patch32`](https://huggingface.co/Xenova/clip-vit-base-patch32) at revision
`d15189d7028b43f1d3e65039190477f6af591c2a`), run with ONNX Runtime on the CPU. It compares the
image with the text prompts in [`prompts.py`](prompts.py): a softmax over all of them gives each
prompt a probability, and a category's score is the sum over its prompts. The neutral prompts
(people, pets, drawings, logos, beaches, red things…) give ordinary pictures somewhere to go.
Nothing leaves the container: the model files are part of the image, and ONNX Runtime's usage
telemetry is switched off.

## API

`POST /v1/check` with `Content-Type: image/png` and a PNG of at most 1 MiB and 1024×1024 pixels
(rim-server sends the 256×256 PNG it stores):

```json
{"scores": {"sexual": 0.0117, "gore": 0.0052, "violence": 0.0068}, "model": "clip-vit-b32-q8@d15189d"}
```

Each score is between 0 and 1, rounded to four places; `model` names the model and its revision.
Other answers carry `{"error": "…"}`:

| Status | When |
|---|---|
| 400 | the body is not a readable PNG |
| 411 | no `Content-Length` |
| 413 | `Content-Length` over 1 MiB (refused before the body is read), or an image over 1024×1024 |
| 415 | another `Content-Type` |
| 503 | the model is still loading |
| 404 | any other path or method |

`GET /healthz` answers `200 ok` once the model is loaded and `503` before; the image's
`HEALTHCHECK` probes it. Each check logs one line with its three scores and its duration; image
data is never logged.

Settings: `PORT` (default 8090) and `MODEL_DIR` (default the script's directory, `/app` in the
image).

## Scores and actions

The service only scores; rim-server decides, from the highest of the three scores:

| Highest score | rim-server | Setting (default) |
|---|---|---|
| at or above the block threshold | refuses the picture with 422, stores nothing | `RIM_AVATAR_CHECK_BLOCK` (0.80) |
| at or above the report threshold | stores it and files an abuse report for the operators | `RIM_AVATAR_CHECK_REPORT` (0.40) |
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
| astronaut | 0.0760 | 0.0640 | 0.0060 |
| camera | 0.1177 | 0.0181 | 0.0772 |
| coffee | 0.2028 | 0.0728 | 0.0433 |
| chelsea (cat) | 0.0117 | 0.0052 | 0.0068 |
| rocket | 0.2290 | 0.0149 | 0.0068 |
| hubble_deep_field | 0.2516 | 0.0139 | 0.0072 |
| coins (greyscale) | **0.8251** | 0.0086 | 0.0059 |
| horse (silhouette) | 0.0553 | 0.0208 | 0.0035 |
| moon | 0.0400 | 0.0167 | 0.0160 |
| page (greyscale scan of text) | **0.6624** | 0.0145 | 0.0022 |
| text (greyscale photo of text) | 0.2119 | 0.1112 | 0.0260 |
| immunohistochemistry (microscope) | **0.8873** | 0.0175 | 0.0066 |
| clock | 0.1687 | 0.0069 | 0.0112 |
| logo | 0.0768 | 0.0967 | 0.0166 |
| grass | 0.1132 | 0.0784 | 0.0169 |
| gravel | 0.3089 | 0.0546 | 0.0118 |
| brick | 0.0072 | 0.0036 | 0.0009 |
| colorwheel | 0.0700 | 0.0567 | 0.0111 |
| retina | 0.1330 | 0.0095 | 0.0099 |
| solid white | 0.3154 | 0.0175 | 0.0171 |
| solid black | 0.1226 | 0.0917 | 0.0454 |
| solid grey | 0.0748 | 0.0161 | 0.0219 |
| solid green | 0.3310 | 0.0211 | 0.0158 |
| solid blue | 0.2244 | 0.0570 | 0.0254 |
| skin tone (solid) | 0.1730 | 0.0303 | 0.0072 |
| pure red | 0.0103 | 0.0121 | 0.0019 |
| dark red | 0.0257 | 0.0289 | 0.0077 |
| gradient black to white | 0.0527 | 0.0095 | 0.0069 |
| gradient red to blue | 0.0768 | 0.0564 | 0.0368 |
| radial red gradient | 0.0002 | 0.0005 | 0.0001 |
| random noise | 0.1224 | 0.0998 | 0.0042 |
| rendered text, black on white | 0.3823 | 0.0073 | 0.0038 |
| rendered text "BLOOD RED", white on red | 0.0031 | **0.6541** | 0.0040 |

Photos of people, pets and plain or red images stay well below 0.40. Greyscale textures and
pages of text are the weak spot of this prompt set: CLIP puts them nearest "a photo of exposed
breasts" and "a photo of exposed genitals", and three of them reach the report threshold, two
the block threshold. Text on red lands on "a pool of blood". The int8 model and the fp32 one
agree on these to within a few hundredths, so this comes from the prompts, not from the
quantization.

## Resources

On an 8-CPU host, with the limits `deploy/compose.yml` sets (1 GB, 2 CPUs):

- image: 371 MB (Python 3.12 slim, ONNX Runtime, NumPy, Pillow, the 89 MB model);
- memory: about 200 MiB resident after 20 checks, 220 MiB after a burst of 8 at a time;
- latency: 35–40 ms per check one at a time, including HTTP; one inference runs at a time on
  2 threads, about 27 checks a second when requests queue;
- start: the model loads in under a second.

## Licence

CLIP's code and weights are released by OpenAI under the MIT licence
([LICENSE](https://github.com/openai/CLIP/blob/main/LICENSE)). Neither Hugging Face model card,
`openai/clip-vit-base-patch32` nor its ONNX conversion `Xenova/clip-vit-base-patch32`, declares a
licence of its own. OpenAI's model card calls deployed use of CLIP out of scope without
task-specific testing; the calibration above is the start of that testing for this prompt set,
and operators should extend it on their own traffic before relying on the block threshold.

The service's own code is under the repository's Apache-2.0 licence.

## Building and testing

```sh
docker build -t rim-imagecheck:local imagecheck
docker run --rm --read-only --tmpfs /tmp -p 8090:8090 rim-imagecheck:local
curl -s --data-binary @picture.png -H 'Content-Type: image/png' http://localhost:8090/v1/check

pip install -r imagecheck/requirements.txt
python -m unittest discover -s imagecheck/tests
```

The build downloads the three model files (the int8 vision model; the fp32 text model and the
tokenizer, which only embed the prompts at build time), checks them against
[`models.sha256`](models.sha256) and runs [`build_prompts.py`](build_prompts.py). The tests need
no model.
