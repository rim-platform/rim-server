"""Draws a message's text as a picture, the way a client shows it.

Braille art is decoded dot for dot; any other text is set in DejaVu Sans Mono. Both end as a
blurred, square RGB picture of at most 256x256 pixels for CLIP.
"""

from PIL import Image, ImageDraw, ImageFilter, ImageFont

# The share of non-whitespace characters that must be braille for a text to be drawn as braille.
BRAILLE_SHARE = 0.5
# Most lines and most characters per line that are drawn; the rest of a text is dropped.
MAX_LINES = 200
MAX_COLUMNS = 400
# The longest side of the picture, in pixels.
MAX_SIDE = 256
# Blur radii for braille and for font text.
BRAILLE_BLUR = 1.0
FONT_BLUR = 2.0
# Font text: line height and margin in pixels.
LINE_HEIGHT = 20
MARGIN = 4

BRAILLE_FIRST = 0x2800
BRAILLE_LAST = 0x28FF
# The (x, y, bit) of each of a braille cell's eight dots.
BRAILLE_DOTS = (
    (0, 0, 0x01),
    (0, 1, 0x02),
    (0, 2, 0x04),
    (1, 0, 0x08),
    (1, 1, 0x10),
    (1, 2, 0x20),
    (0, 3, 0x40),
    (1, 3, 0x80),
)


def is_braille(text: str) -> bool:
    """Whether more than BRAILLE_SHARE of the non-whitespace characters of `text` are braille."""
    marks = [char for char in text if not char.isspace()]
    if not marks:
        return False
    braille = sum(BRAILLE_FIRST <= ord(char) <= BRAILLE_LAST for char in marks)
    return braille > BRAILLE_SHARE * len(marks)


def lines_of(text: str) -> list[str]:
    """The lines of `text` that are drawn: at most MAX_LINES of at most MAX_COLUMNS characters."""
    return [line[:MAX_COLUMNS] for line in text.splitlines()[:MAX_LINES]]


def draw_braille(text: str) -> Image.Image:
    """`text` as a greyscale picture: each character a 2x4 cell with its raised dots black."""
    lines = lines_of(text)
    width = 2 * max((len(line) for line in lines), default=1)
    image = Image.new("L", (max(width, 1), max(4 * len(lines), 1)), 255)
    pixels = image.load()
    for row, line in enumerate(lines):
        for column, char in enumerate(line):
            code = ord(char) - BRAILLE_FIRST
            if not 0 <= code <= BRAILLE_LAST - BRAILLE_FIRST:
                continue
            for x, y, bit in BRAILLE_DOTS:
                if code & bit:
                    pixels[2 * column + x, 4 * row + y] = 0
    return image


def draw_font(text: str, font: ImageFont.FreeTypeFont) -> Image.Image:
    """`text` as a greyscale picture: black `font` text on white with a margin."""
    lines = lines_of(text)
    width = max((font.getlength(line) for line in lines), default=0)
    image = Image.new(
        "L",
        (int(width) + 2 * MARGIN, LINE_HEIGHT * max(len(lines), 1) + 2 * MARGIN),
        255,
    )
    draw = ImageDraw.Draw(image)
    for row, line in enumerate(lines):
        draw.text((MARGIN, MARGIN + LINE_HEIGHT * row), line, font=font, fill=0)
    return image


def picture(text: str, font: ImageFont.FreeTypeFont) -> Image.Image:
    """`text` as a square RGB picture of at most MAX_SIDE pixels, black on white."""
    if is_braille(text):
        image = draw_braille(text).filter(ImageFilter.GaussianBlur(BRAILLE_BLUR))
    else:
        image = draw_font(text, font).filter(ImageFilter.GaussianBlur(FONT_BLUR))
    image.thumbnail((MAX_SIDE, MAX_SIDE), Image.Resampling.LANCZOS)
    side = max(image.size)
    square = Image.new("L", (side, side), 255)
    square.paste(image, ((side - image.width) // 2, (side - image.height) // 2))
    return square.convert("RGB")
