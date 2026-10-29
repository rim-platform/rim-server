"""The text prompts CLIP compares text art with, grouped by category.

Text art is drawn as a picture (textart.py) and scored for sexual content against prompts
written for drawings, line art, dotted and edge renderings and text art. The `other` prompts
give harmless drawings, dotted renderings and text somewhere to go, so that the softmax over all
prompts does not push their probability onto the sexual ones.
"""

PROMPTS: dict[str, list[str]] = {
    "sexual": [
        "an explicit sexual drawing",
        "a drawing of a naked woman",
        "a drawing of a naked man",
        "hentai",
        "a pornographic illustration",
        "a nude anime girl",
        "a drawing of people having sex",
        "a drawing of genitals",
        "an erotic sketch",
    ],
    "other": [
        "a drawing of a person",
        "a portrait",
        "an anime character",
        "a black and white sketch",
        "a line drawing",
        "a drawing of an animal",
        "a drawing of food",
        "a landscape",
        "text",
        "an icon",
        "a logo",
        "a table",
        "source code",
        "a pattern",
        "a photo of an object",
        "a painting",
        "a line drawing of a person",
        "a sketch of a face",
        "a drawing of a clothed person",
        "a portrait drawing",
        "a drawing of a person playing sports",
        "a drawing of a surfer",
        "a manga panel",
        "a cartoon girl in school uniform",
        "a drawing of a person in a dress",
        "a drawing of a man in a suit",
        "a drawing of hands",
        "a dotted halftone picture",
        "a stippled drawing",
        "an edge-detected photo",
        "a noisy black and white image",
        "a dithered image",
        "a pixel art picture",
        "a drawing of a mug",
        "a drawing of a bicycle",
        "a drawing of a building",
        "a drawing of waves",
        "a drawing of a mountain",
        "a drawing of a plant",
        "a drawing of headphones",
        "a drawing of a book",
        "a drawing of a machine",
        "ascii art letters",
        "a banner of large letters",
        "a terminal screenshot",
        "a log file",
        "a spreadsheet",
        "a box diagram",
        "an emoticon",
        "a drawing of a cup",
        "a drawing of a musical instrument",
        "a drawing of a toy",
        "a drawing of a bird",
        "a drawing of a pet",
        "a drawing of a household object",
        "a drawing of a vehicle",
        "a drawing of a flower",
    ],
}

# The categories the score is reported for.
SCORED: tuple[str, ...] = ("sexual",)


def rows() -> list[tuple[str, str]]:
    """Every prompt with its category, in a fixed order: (category, prompt)."""
    return [(category, prompt) for category, prompts in PROMPTS.items() for prompt in prompts]
