"""Tests of drawing text art as a picture; they need no model."""

import os
import sys
import unittest

from PIL import Image, ImageFont

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import textart
from textart import MAX_LINES, draw_braille, is_braille, picture

# DejaVu Sans Mono when installed, else PIL's built-in font; the tests only check geometry.
FONT_PATH = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"


def font():
    if os.path.exists(FONT_PATH):
        return ImageFont.truetype(FONT_PATH, 16)
    return ImageFont.load_default(16)


def black(image):
    """The coordinates of the black pixels of a greyscale image."""
    width, height = image.size
    return {(x, y) for x in range(width) for y in range(height) if image.getpixel((x, y)) == 0}


class IsBrailleTest(unittest.TestCase):
    def test_braille_block_is_braille(self):
        self.assertTrue(is_braille("⣿⣿⣿\n⠁⠂⠄"))

    def test_ascii_art_is_not(self):
        self.assertFalse(is_braille("+--+\n|ab|\n+--+"))

    def test_mixed_text_below_half_is_not(self):
        self.assertFalse(is_braille("⣿⣿ hello world"))

    def test_whitespace_does_not_count(self):
        self.assertTrue(is_braille("⣿   \n   ⣿ a"))


class DrawBrailleTest(unittest.TestCase):
    def test_one_dot(self):
        image = draw_braille("⠁")
        self.assertEqual((image.mode, image.size), ("L", (2, 4)))
        self.assertEqual(black(image), {(0, 0)})

    def test_all_dots(self):
        self.assertEqual(len(black(draw_braille("⣿"))), 8)

    def test_dot_positions(self):
        for bit, position in (
            (0x01, (0, 0)), (0x02, (0, 1)), (0x04, (0, 2)), (0x08, (1, 0)),
            (0x10, (1, 1)), (0x20, (1, 2)), (0x40, (0, 3)), (0x80, (1, 3)),
        ):  # fmt: skip
            self.assertEqual(black(draw_braille(chr(0x2800 + bit))), {position}, hex(bit))

    def test_size_follows_the_longest_line_and_the_line_count(self):
        self.assertEqual(draw_braille("⠁⠁⠁⠁⠁\n⠁\n⠁⠁").size, (10, 12))

    def test_other_characters_leave_a_blank_cell(self):
        self.assertEqual(black(draw_braille("a⠁")), {(2, 0)})


class PictureTest(unittest.TestCase):
    def test_braille_picture_is_a_square_rgb_image(self):
        image = picture("⣿⣿⣿⣿\n⣿⣿⣿⣿" * 30, font())
        self.assertEqual(image.mode, "RGB")
        self.assertEqual(image.width, image.height)
        self.assertLessEqual(image.width, 256)

    def test_ascii_picture_is_a_square_rgb_image(self):
        image = picture("+----+\n| ab |\n+----+", font())
        self.assertEqual(image.mode, "RGB")
        self.assertEqual(image.width, image.height)
        self.assertLessEqual(image.width, 256)

    def test_limits_lines_and_columns(self):
        braille = draw_braille("\n".join(["⣿" * 1000] * 1000))
        self.assertEqual(braille.size, (2 * textart.MAX_COLUMNS, 4 * MAX_LINES))
        drawn = textart.draw_font("\n".join(["a" * 1000] * 1000), font())
        self.assertLessEqual(drawn.height, 20 * MAX_LINES + 8)
        self.assertLessEqual(drawn.width, 400 * 16 + 8)


if __name__ == "__main__":
    unittest.main()
