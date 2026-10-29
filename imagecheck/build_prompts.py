"""Computes the prompt embeddings the service compares images with, at image build time.

    python build_prompts.py <text_model.onnx> <tokenizer.json> <module> <out.npz>

<module> is `prompts` (profile pictures) or `text_prompts` (text art). Tokenizes every prompt in
that module to CLIP's 77 tokens (start and end tokens as tokenizer.json adds them, padded with its
pad token), runs the fp32 text model, L2-normalizes the embeddings and writes them with each row's
category and the logit scale.
"""

import importlib
import sys

import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

# CLIP's context length.
CONTEXT = 77
# CLIP ViT-B/32's trained logit scale, exp(4.6052).
LOGIT_SCALE = 100.0


def tokenize(tokenizer: Tokenizer, texts: list[str]) -> np.ndarray:
    """The token ids of `texts`, one row of CONTEXT ids each, as int64."""
    pad = "<|endoftext|>"
    tokenizer.enable_truncation(max_length=CONTEXT)
    tokenizer.enable_padding(length=CONTEXT, pad_id=tokenizer.token_to_id(pad), pad_token=pad)
    return np.array([enc.ids for enc in tokenizer.encode_batch(texts)], dtype=np.int64)


def main(model_path: str, tokenizer_path: str, module: str, out_path: str) -> None:
    table = importlib.import_module(module).rows()
    ids = tokenize(Tokenizer.from_file(tokenizer_path), [prompt for _, prompt in table])
    session = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
    (embeds,) = session.run(["text_embeds"], {"input_ids": ids})
    embeds = embeds / np.linalg.norm(embeds, axis=1, keepdims=True)
    np.savez(
        out_path,
        embeds=embeds.astype(np.float32),
        labels=np.array([category for category, _ in table]),
        logit_scale=np.float32(LOGIT_SCALE),
    )
    print(f"{len(table)} prompts, embeddings {embeds.shape} written to {out_path}")


if __name__ == "__main__":
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    main(*sys.argv[1:])
