"""The text prompts CLIP compares an image with, grouped by category.

CLIP scores gore and violence. The sexual prompts stay in the softmax without being reported,
since the classifier scores sexual content: they take the probability of nude pictures that
would otherwise go to gore or violence. The neutral prompts give ordinary pictures somewhere to
go, so that the softmax over all prompts does not push their probability onto a category.
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
        "a black and white photo",
        "a close-up texture",
        "a microscope image",
        "a page of text",
        "a photo of coins",
        "a plain colour background",
        "a blurry photo",
        "a screenshot",
        "a photo of an object",
        "a photo of a building",
        "a photo of plants",
        "a photo of the sky",
    ],
}

# The categories CLIP's score is reported for, in the order of the answer.
SCORED: tuple[str, ...] = ("gore", "violence")


def rows() -> list[tuple[str, str]]:
    """Every prompt with its category, in a fixed order: (category, prompt)."""
    return [(category, prompt) for category, prompts in PROMPTS.items() for prompt in prompts]
