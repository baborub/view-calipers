use std::io::Cursor;
use image::{DynamicImage, ImageDecoder, ImageReader, ImageResult, metadata::Orientation};

/// Apply the file's display orientation before resizing or uploading pixels.
/// Missing or unreadable metadata must not prevent decoding the actual image.
pub fn decode(bytes: &[u8]) -> ImageResult<DynamicImage> {
    let mut decoder = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

#[cfg(test)]
pub(crate) fn jpeg_with_orientation(value: u16) -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(24, 16, |x, y| image::Rgb([
        (x * 10) as u8, (y * 15) as u8, ((x + y) * 6) as u8,
    ]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&pixels).unwrap();
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
            assert_eq!(decode(&jpeg_with_orientation(value)).unwrap().to_rgba8(), expected.to_rgba8(), "EXIF {value}");
        }
    }

    #[test]
    fn missing_or_invalid_orientation_keeps_pixels_unchanged() {
        let invalid = jpeg_with_orientation(99);
        assert_eq!(decode(&invalid).unwrap().to_rgba8(), image::load_from_memory(&invalid).unwrap().to_rgba8());
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(9, 5).write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert_eq!(decode(png.get_ref()).unwrap().to_rgba8(), image::load_from_memory(png.get_ref()).unwrap().to_rgba8());
        assert!(decode(b"not an image").is_err());
    }
}
