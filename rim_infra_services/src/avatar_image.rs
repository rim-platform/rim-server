//! Profile pictures re-encoded on the server: whatever was uploaded is stored as a square
//! static PNG without metadata.

use std::io::Cursor;

use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, ImageError, ImageFormat, ImageReader, Limits};
use rim_core::user::avatar::AvatarFormat;

/// The width and height of a stored avatar, in pixels.
pub const AVATAR_SIDE: u32 = 256;
/// The largest width or height of an uploaded image, in pixels.
pub const MAX_SOURCE_SIDE: u32 = 4096;
/// The most memory decoding one uploaded image may allocate, in bytes.
pub const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;

/// Why an uploaded image could not be re-encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarImageError {
    /// The bytes are not a readable image of the declared format.
    Unreadable,
    /// The image is wider or taller than [`MAX_SOURCE_SIDE`], or decoding it would allocate
    /// more than [`MAX_DECODE_BYTES`].
    TooLarge,
}

/// The decoder of `format`.
fn image_format(format: AvatarFormat) -> ImageFormat {
    match format {
        AvatarFormat::Png => ImageFormat::Png,
        AvatarFormat::Jpeg => ImageFormat::Jpeg,
        AvatarFormat::Gif => ImageFormat::Gif,
        AvatarFormat::Webp => ImageFormat::WebP,
    }
}

/// `bytes` decoded as `format` (never guessed), its first frame centre-cropped to a square,
/// resized to [`AVATAR_SIDE`]×[`AVATAR_SIDE`] and encoded as an RGBA PNG with no metadata.
///
/// # Errors
/// [`AvatarImageError::TooLarge`] past the decoder's limits, [`AvatarImageError::Unreadable`]
/// for anything else the decoder or the encoder refuses.
pub fn normalize_avatar(bytes: &[u8], format: AvatarFormat) -> Result<Vec<u8>, AvatarImageError> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), image_format(format));
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| match e {
        ImageError::Limits(_) => AvatarImageError::TooLarge,
        _ => AvatarImageError::Unreadable,
    })?;

    let side = image.width().min(image.height());
    if side == 0 {
        return Err(AvatarImageError::Unreadable);
    }
    let square = image.crop_imm(
        (image.width() - side) / 2,
        (image.height() - side) / 2,
        side,
        side,
    );
    let pixels = square
        .resize_exact(AVATAR_SIDE, AVATAR_SIDE, FilterType::CatmullRom)
        .to_rgba8();

    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(
            pixels.as_raw(),
            AVATAR_SIDE,
            AVATAR_SIDE,
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| AvatarImageError::Unreadable)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use image::codecs::gif::GifEncoder;
    use image::{DynamicImage, Frame, Rgba, RgbaImage};

    use super::*;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const GREEN: Rgba<u8> = Rgba([0, 255, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    /// `image` encoded as `format`; JPEG without its alpha channel.
    fn encode(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let image = match format {
            ImageFormat::Jpeg => {
                DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(image.clone()).to_rgb8())
            }
            _ => DynamicImage::ImageRgba8(image.clone()),
        };
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).expect("encode");
        bytes.into_inner()
    }

    /// The decoded avatar, after checking it is a PNG of [`AVATAR_SIDE`] square.
    fn decoded(png: &[u8]) -> RgbaImage {
        assert_eq!(
            image::guess_format(png).expect("a format"),
            ImageFormat::Png
        );
        let image = image::load_from_memory_with_format(png, ImageFormat::Png)
            .expect("decode")
            .to_rgba8();
        assert_eq!(image.dimensions(), (AVATAR_SIDE, AVATAR_SIDE));
        image
    }

    /// Whether `pixel` is `expected`, give or take what JPEG and resampling change.
    fn near(pixel: &Rgba<u8>, expected: Rgba<u8>) -> bool {
        pixel
            .0
            .iter()
            .zip(expected.0)
            .all(|(got, want)| got.abs_diff(want) <= 16)
    }

    /// The chunk types of a PNG file, in order.
    fn chunk_types(png: &[u8]) -> Vec<String> {
        let mut types = Vec::new();
        let mut at = 8;
        while at + 8 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().expect("length")) as usize;
            types.push(String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned());
            at += 12 + len;
        }
        types
    }

    #[test]
    fn every_accepted_format_comes_out_a_square_png() {
        let source = RgbaImage::from_pixel(300, 300, RED);
        for (format, image_format) in [
            (AvatarFormat::Png, ImageFormat::Png),
            (AvatarFormat::Jpeg, ImageFormat::Jpeg),
            (AvatarFormat::Gif, ImageFormat::Gif),
            (AvatarFormat::Webp, ImageFormat::WebP),
        ] {
            let png = normalize_avatar(&encode(&source, image_format), format)
                .unwrap_or_else(|e| panic!("{format:?}: {e:?}"));
            let image = decoded(&png);
            assert!(near(image.get_pixel(128, 128), RED), "{format:?}");
        }
    }

    #[test]
    fn an_animated_gif_keeps_only_its_first_frame() {
        let mut gif = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut gif);
            encoder
                .encode_frames([
                    Frame::new(RgbaImage::from_pixel(64, 64, RED)),
                    Frame::new(RgbaImage::from_pixel(64, 64, BLUE)),
                ])
                .expect("encode");
        }
        let png = normalize_avatar(&gif, AvatarFormat::Gif).expect("normalized");
        let image = decoded(&png);
        assert!(image.pixels().all(|p| near(p, RED)));
    }

    #[test]
    fn a_wide_image_is_cropped_to_its_centre() {
        // Red on the left quarter, blue on the right quarter, green between.
        let source = RgbaImage::from_fn(600, 300, |x, _| match x {
            0..150 => RED,
            450.. => BLUE,
            _ => GREEN,
        });
        let png = normalize_avatar(&encode(&source, ImageFormat::Png), AvatarFormat::Png)
            .expect("normalized");
        let image = decoded(&png);
        for x in [0, 128, AVATAR_SIDE - 1] {
            assert!(near(image.get_pixel(x, 128), GREEN), "x {x}");
        }
    }

    #[test]
    fn an_image_past_the_side_limit_is_too_large() {
        let source = RgbaImage::from_pixel(MAX_SOURCE_SIDE + 1, 10, RED);
        assert_eq!(
            normalize_avatar(&encode(&source, ImageFormat::Png), AvatarFormat::Png),
            Err(AvatarImageError::TooLarge)
        );
    }

    #[test]
    fn bytes_that_are_not_an_image_are_unreadable() {
        let noise: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        for format in AvatarFormat::ALL {
            assert_eq!(
                normalize_avatar(&noise, format),
                Err(AvatarImageError::Unreadable),
                "{format:?}"
            );
        }
        let mut truncated = encode(&RgbaImage::from_pixel(64, 64, RED), ImageFormat::Png);
        truncated.truncate(40);
        assert_eq!(
            normalize_avatar(&truncated, AvatarFormat::Png),
            Err(AvatarImageError::Unreadable)
        );
        // A PNG read as a JPEG is not guessed back into a PNG.
        let png = encode(&RgbaImage::from_pixel(64, 64, RED), ImageFormat::Png);
        assert_eq!(
            normalize_avatar(&png, AvatarFormat::Jpeg),
            Err(AvatarImageError::Unreadable)
        );
    }

    #[test]
    fn the_output_carries_no_metadata_chunks() {
        // A PNG with a tEXt chunk after its header.
        let plain = encode(&RgbaImage::from_pixel(32, 32, RED), ImageFormat::Png);
        let header_end = 8 + 12 + 13;
        let mut text = b"tEXtComment\0secret".to_vec();
        let mut crc = flate2::Crc::new();
        crc.update(&text);
        let mut chunk = ((text.len() - 4) as u32).to_be_bytes().to_vec();
        chunk.append(&mut text);
        chunk.extend_from_slice(&crc.sum().to_be_bytes());
        let png = [&plain[..header_end], &chunk, &plain[header_end..]].concat();
        assert!(chunk_types(&png).contains(&"tEXt".to_string()));

        let normalized = normalize_avatar(&png, AvatarFormat::Png).expect("normalized");
        assert_eq!(
            chunk_types(&normalized).first().map(String::as_str),
            Some("IHDR")
        );
        assert_eq!(
            chunk_types(&normalized).last().map(String::as_str),
            Some("IEND")
        );
        assert!(
            chunk_types(&normalized)
                .iter()
                .all(|t| ["IHDR", "IDAT", "IEND"].contains(&t.as_str())),
            "{:?}",
            chunk_types(&normalized)
        );
    }
}
