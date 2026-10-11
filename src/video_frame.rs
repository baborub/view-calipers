//! Decoder output has one explicit layout and color convention, shared with the RAM cache.
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Format {
    Rgb24,
    Yuv420,
}
impl Format {
    pub fn current() -> Self {
        if std::env::var_os("VIEW_CALIPERS_MEDIA_LEGACY").is_some() {
            Self::Rgb24
        } else {
            Self::Yuv420
        }
    }
    pub fn pixel_format(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Yuv420 => "yuv420p",
        }
    }
    pub fn length(self, [w, h]: [usize; 2]) -> Option<usize> {
        if w == 0 || h == 0 {
            return None;
        }
        let y = w.checked_mul(h)?;
        match self {
            Self::Rgb24 => y.checked_mul(3),
            Self::Yuv420 => {
                y.checked_add(w.div_ceil(2).checked_mul(h.div_ceil(2))?.checked_mul(2)?)
            }
        }
    }
    /// Normalize both range and matrix in FFmpeg; the shader always consumes limited BT.709.
    pub fn scale(self, [w, h]: [usize; 2]) -> String {
        match self {
            Self::Rgb24 => format!("scale={w}:{h}"),
            Self::Yuv420 => {
                format!("scale={w}:{h}:out_color_matrix=bt709:out_range=tv,format=yuv420p")
            }
        }
    }
}

#[derive(Clone)]
pub struct Frame {
    pub size: [usize; 2],
    pub pixels: Arc<Vec<u8>>,
    pub format: Format,
}
impl Frame {
    pub fn new(size: [usize; 2], pixels: Arc<Vec<u8>>, format: Format) -> Self {
        assert_eq!(format.length(size), Some(pixels.len()));
        Self {
            size,
            pixels,
            format,
        }
    }
    /// Headless tests / legacy diagnostic path only. Native playback uploads YUV unchanged.
    pub fn color_image(&self) -> egui::ColorImage {
        if self.format == Format::Rgb24 {
            return egui::ColorImage::from_rgb(self.size, &self.pixels);
        }
        let [w, h] = self.size;
        let y_len = w * h;
        let cw = w.div_ceil(2);
        let c_len = cw * h.div_ceil(2);
        let pixels = (0..y_len)
            .map(|i| {
                let c = (i / w / 2) * cw + i % w / 2;
                let rgb = bt709(
                    self.pixels[i],
                    self.pixels[y_len + c],
                    self.pixels[y_len + c_len + c],
                );
                egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
            })
            .collect();
        egui::ColorImage {
            size: self.size,
            pixels,
        }
    }
}
pub fn bt709(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = (y as f32 - 16.0) / 219.0;
    let u = (u as f32 - 128.0) / 224.0;
    let v = (v as f32 - 128.0) / 224.0;
    [
        y + 1.5748 * v,
        y - 0.187324 * u - 0.468124 * v,
        y + 1.8556 * u,
    ]
    .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planar_layout_handles_odd_portrait_dimensions_and_overflow() {
        assert_eq!(Format::Yuv420.length([960, 540]), Some(777600));
        assert_eq!(Format::Yuv420.length([3, 5]), Some(27));
        assert_eq!(Format::Yuv420.length([usize::MAX, 2]), None);
        assert_eq!(Format::Yuv420.length([0, 5]), None);
        let frame = Frame::new(
            [3, 5],
            Arc::new([vec![235; 15], vec![128; 12]].concat()),
            Format::Yuv420,
        );
        assert!(frame
            .color_image()
            .pixels
            .iter()
            .all(|p| *p == egui::Color32::WHITE));
    }
    #[test]
    fn limited_range_black_white_and_saturated_red() {
        assert_eq!(bt709(16, 128, 128), [0, 0, 0]);
        assert_eq!(bt709(235, 128, 128), [255, 255, 255]);
        let red = bt709(63, 102, 240);
        assert!(red[0] >= 254 && red[1] <= 1 && red[2] <= 1);
    }
    #[test]
    fn ffmpeg_normalizes_bt601_bt709_and_full_range_for_the_shader() {
        let root =
            std::env::temp_dir().join(format!("view-calipers-yuv-color-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (matrix, range) in [("bt601", "tv"), ("bt709", "tv"), ("bt709", "pc")] {
            let path = root.join(format!("{matrix}-{range}.mkv"));
            let filter =
                format!("scale=96:64:out_color_matrix={matrix}:out_range={range},format=yuv420p");
            let metadata = if matrix == "bt601" {
                "smpte170m"
            } else {
                "bt709"
            };
            let output = crate::video::command("ffmpeg")
                .args([
                    "-y",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=96x64:rate=24",
                    "-frames:v",
                    "1",
                    "-vf",
                    &filter,
                    "-c:v",
                    "ffv1",
                    "-colorspace",
                    metadata,
                    "-color_range",
                    range,
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let extract = |format: Format| {
                let filter = format.scale([96, 64]);
                let output = crate::video::command("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&path)
                    .args([
                        "-frames:v",
                        "1",
                        "-vf",
                        &filter,
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        format.pixel_format(),
                        "pipe:1",
                    ])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                Frame::new([96, 64], Arc::new(output.stdout), format).color_image()
            };
            let actual = extract(Format::Yuv420);
            let reference = extract(Format::Rgb24);
            let error = actual
                .pixels
                .iter()
                .zip(reference.pixels)
                .map(|(a, b)| {
                    a.to_array()[..3]
                        .iter()
                        .zip(&b.to_array()[..3])
                        .map(|(a, b)| a.abs_diff(*b) as u64)
                        .sum::<u64>()
                })
                .sum::<u64>() as f64
                / (96.0 * 64.0 * 3.0);
            assert!(error < 5.0, "{matrix} {range} color error {error}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
