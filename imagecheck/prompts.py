"""The text prompts an image is compared with, grouped by category.

The categories other than `neutral` are the ones the service scores. The neutral prompts give
ordinary pictures somewhere to go, so that the softmax over all prompts does not push their
probability onto a scored category.
"""

PROMPTS: dict[str, list[str]] = {
    "sexual": [
        "a photo of a naked person",
        "explicit nudity",
        "a pornographic image",
        "a photo of exposed genitals",
        "a photo of exposed breasts",
        "people having sex",
    ],
    "gore": [
        "a photo of a corpse",
        "a dead body",
        "a bloody wound",
        "graphic gore",
        "a mutilated body",
        "a pool of blood",
    ],
    "violence": [
        "a person being beaten",
        "a person being shot",
        "a photo of a violent attack",
        "a person holding a knife to someone's throat",
    ],
    "neutral": [
        "a photo of a person",
        "a portrait photo of a face",
        "a selfie",
        "a cartoon avatar",
        "an anime character",
        "a photo of a cat",
        "a photo of a dog",
        "a landscape photo",
        "a company logo",
        "a drawing",
        "a photo of food",
        "a person in a swimsuit",
        "people at the beach",
        "a photo of a red object",
        "a person with red hair",
        "text on a plain background",
        "a meme",
        "an abstract pattern",
        "a photo of a car",
        "a person in a costume",
        "a medical illustration",
        "a video game screenshot",
    ],
}

# The categories a check returns a score for, in the order of the answer.
SCORED: tuple[str, ...] = ("sexual", "gore", "violence")


def rows() -> list[tuple[str, str]]:
    """Every prompt with its category, in a fixed order: (category, prompt)."""
    return [(category, prompt) for category, prompts in PROMPTS.items() for prompt in prompts]
