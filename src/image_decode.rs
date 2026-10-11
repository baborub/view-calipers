use image::{metadata::Orientation, DynamicImage, ImageDecoder, ImageReader, ImageResult};
use std::io::Cursor;

/// Apply the file's display orientation before resizing or uploading pixels.
/// Missing or unreadable metadata must not prevent decoding the actual image.
pub fn decode(bytes: &[u8]) -> ImageResult<DynamicImage> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// JPEG's reduced IDCT avoids constructing a full-resolution raster for a thumbnail.
/// Unsupported JPEG variants use the regular decoder, as do all other formats.
pub fn thumbnail(bytes: &[u8], width: u32, height: u32) -> ImageResult<DynamicImage> {
    if bytes.starts_with(&[0xff, 0xd8]) {
        if let Some(image) = scaled_jpeg(bytes, width, height) {
            return Ok(image.thumbnail(width.min(image.width()), height.min(image.height())));
        }
    }
    let image = decode(bytes)?;
    Ok(image.thumbnail(width.min(image.width()), height.min(image.height())))
}

fn scaled_jpeg(bytes: &[u8], width: u32, height: u32) -> Option<DynamicImage> {
    let mut metadata = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = metadata.orientation().unwrap_or(Orientation::NoTransforms);
    drop(metadata);
    let swap = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    let (width, height) = if swap {
        (height, width)
    } else {
        (width, height)
    };
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.read_info().ok()?;
    // CMYK and 16-bit/lossless images keep the established color conversion path.
    let format = decoder.info()?.pixel_format;
    if !matches!(
        format,
        jpeg_decoder::PixelFormat::RGB24 | jpeg_decoder::PixelFormat::L8
    ) {
        return None;
    }
    decoder
        .scale(
            width.clamp(1, u16::MAX as u32) as u16,
            height.clamp(1, u16::MAX as u32) as u16,
        )
        .ok()?;
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let mut image = match format {
        jpeg_decoder::PixelFormat::RGB24 => DynamicImage::ImageRgb8(image::RgbImage::from_raw(
            info.width.into(),
            info.height.into(),
            pixels,
        )?),
        jpeg_decoder::PixelFormat::L8 => DynamicImage::ImageLuma8(image::GrayImage::from_raw(
            info.width.into(),
            info.height.into(),
            pixels,
        )?),
        _ => return None,
    };
    image.apply_orientation(orientation);
    Some(image)
}

#[cfg(test)]
pub(crate) fn jpeg_with_orientation(value: u16) -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(24, 16, |x, y| {
        image::Rgb([(x * 10) as u8, (y * 15) as u8, ((x + y) * 6) as u8])
    });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode_image(&pixels)
        .unwrap();
    let mut exif = b"Exif\0\0II".to_vec();
    exif.extend(42_u16.to_le_bytes());
    exif.extend(8_u32.to_le_bytes());
    exif.extend(1_u16.to_le_bytes());
    exif.extend(0x0112_u16.to_le_bytes());
    exif.extend(3_u16.to_le_bytes());
    exif.extend(1_u32.to_le_bytes());
    exif.extend(value.to_le_bytes());
    exif.extend(0_u16.to_le_bytes());
    exif.extend(0_u32.to_le_bytes());
    let mut tagged = jpeg[..2].to_vec();
    tagged.extend([0xff, 0xe1]);
    tagged.extend(((exif.len() + 2) as u16).to_be_bytes());
    tagged.extend(exif);
    tagged.extend(&jpeg[2..]);
    tagged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_thumbnail_preserves_all_exif_orientations_and_fallback_formats() {
        let original = scaled_jpeg(&jpeg_with_orientation(1), 320, 240).unwrap();
        for value in 1..=8 {
            let mut expected = original.clone();
            expected.apply_orientation(Orientation::from_exif(value as u8).unwrap());
            assert_eq!(
                thumbnail(&jpeg_with_orientation(value), 320, 240)
                    .unwrap()
                    .to_rgba8(),
                expected.to_rgba8(),
                "EXIF {value}"
            );
        }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(640, 480)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(
            thumbnail(png.get_ref(), 320, 240).unwrap().to_rgba8(),
            decode(png.get_ref())
                .unwrap()
                .thumbnail(320, 240)
                .to_rgba8()
        );
        assert!(thumbnail(b"broken", 320, 240).is_err());
    }

    #[test]
    #[ignore = "manual release benchmark; prints timings, no hardware-dependent assertions"]
    fn benchmark_large_jpeg_thumbnail() {
        let pixels = image::RgbImage::from_fn(6000, 4000, |x, y| {
            image::Rgb([(x / 24) as u8, (y / 16) as u8, ((x + y) / 40) as u8])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&pixels)
            .unwrap();
        let reduced = scaled_jpeg(&bytes, 320, 240).unwrap();
        assert_eq!((reduced.width(), reduced.height()), (750, 500));
        for (label, fast) in [("full decode", false), ("reduced decode", true)] {
            let start = std::time::Instant::now();
            for _ in 0..5 {
                let image = if fast {
                    thumbnail(&bytes, 320, 240).unwrap()
                } else {
                    decode(&bytes).unwrap().thumbnail(320, 240)
                };
                assert_eq!((image.width(), image.height()), (320, 213));
                std::hint::black_box(image);
            }
            println!(
                "24 MP JPEG {label}: {:.1} ms/thumbnail",
                start.elapsed().as_secs_f64() * 200.0
            );
        }
    }

    #[test]
    fn exif_all_eight_orientations_rotate_and_mirror_pixels() {
        let original = image::load_from_memory(&jpeg_with_orientation(1)).unwrap();
        for value in 1..=8 {
            let expected = match value {
                1 => original.clone(),
                2 => original.fliph(),
                3 => original.rotate180(),
                4 => original.flipv(),
                5 => original.rotate90().fliph(),
                6 => original.rotate90(),
                7 => original.rotate270().fliph(),
                8 => original.rotate270(),
                _ => unreachable!(),
            };
            assert_eq!(
                decode(&jpeg_with_orientation(value)).unwrap().to_rgba8(),
                expected.to_rgba8(),
                "EXIF {value}"
            );
        }
    }

    #[test]
    fn missing_or_invalid_orientation_keeps_pixels_unchanged() {
        let invalid = jpeg_with_orientation(99);
        assert_eq!(
            decode(&invalid).unwrap().to_rgba8(),
            image::load_from_memory(&invalid).unwrap().to_rgba8()
        );
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(9, 5)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(
            decode(png.get_ref()).unwrap().to_rgba8(),
            image::load_from_memory(png.get_ref()).unwrap().to_rgba8()
        );
        assert!(decode(b"not an image").is_err());
    }
}
