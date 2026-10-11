//! Opt-in release diagnostic: serialized upload + draw, ending in glFinish.
//! This deliberately excludes decoding, UI scheduling, swap/vsync and monitor latency.
use super::*;
use std::{path::Path, time::Instant};

fn quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}
fn summary(mut values: Vec<f64>) -> String {
    values.sort_by(f64::total_cmp);
    format!(
        "{{\"medianMs\":{},\"p95Ms\":{},\"samples\":{:?}}}",
        values[values.len() / 2],
        values[((values.len() as f64 * 0.95).ceil() as usize - 1).min(values.len() - 1)],
        values
    )
}
unsafe fn draw(
    gl: &glow::Context,
    texture: &Texture,
    painter: &mut eframe::egui_glow::Painter,
    size: [usize; 2],
) {
    let rect =
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
    match texture {
        Texture::Native(t) => {
            gl.viewport(0, 0, size[0] as i32, size[1] as i32);
            gl.disable(glow::SCISSOR_TEST);
            let owner = t.owner.upgrade().unwrap();
            let state = owner.state.lock().unwrap();
            state.program.as_ref().unwrap().paint(gl, t, rect, rect);
        }
        Texture::Managed(t) => {
            let mut mesh = egui::epaint::Mesh::with_texture(t.id());
            mesh.add_rect_with_uv(
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            painter.paint_primitives(
                [size[0] as u32, size[1] as u32],
                1.0,
                &[egui::ClippedPrimitive {
                    clip_rect: rect,
                    primitive: egui::epaint::Primitive::Mesh(mesh),
                }],
            );
        }
    }
}
unsafe fn flush_managed(ctx: &egui::Context, painter: &mut eframe::egui_glow::Painter) {
    let delta = ctx.tex_manager().write().take_delta();
    for (id, image) in delta.set {
        painter.set_texture(id, &image);
    }
    for id in delta.free {
        painter.free_texture(id);
    }
}
unsafe fn read_rgb(gl: &glow::Context, [w, h]: [usize; 2]) -> Vec<u8> {
    let mut rgba = vec![0; w * h * 4];
    gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
    gl.read_pixels(
        0,
        0,
        w as i32,
        h as i32,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelPackData::Slice(&mut rgba),
    );
    let mut result = Vec::with_capacity(w * h * 3);
    for y in (0..h).rev() {
        for x in 0..w {
            result.extend_from_slice(&rgba[(y * w + x) * 4..(y * w + x) * 4 + 3]);
        }
    }
    result
}
fn yuv_fixture([w, h]: [usize; 2]) -> Frame {
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut pixels = vec![0; Format::Yuv420.length([w, h]).unwrap()];
    let colors = [
        [16, 128, 128],
        [235, 128, 128],
        [63, 102, 240],
        [32, 240, 118],
    ];
    for y in 0..h {
        for x in 0..w {
            pixels[y * w + x] = colors[(x * 2 / w) + (y * 2 / h) * 2][0];
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            let c = colors[(x * 2 / cw) + (y * 2 / ch) * 2];
            pixels[w * h + y * cw + x] = c[1];
            pixels[w * h + cw * ch + y * cw + x] = c[2];
        }
    }
    Frame::new([w, h], Arc::new(pixels), Format::Yuv420)
}
fn rgb_of(frame: &Frame) -> Vec<u8> {
    frame
        .color_image()
        .pixels
        .into_iter()
        .flat_map(|p| p.to_array().into_iter().take(3))
        .collect()
}

pub fn run(ctx: &egui::Context, path: &Path) {
    let result = unsafe { measure(ctx) };
    let report = match result {
        Ok(report) => report,
        Err(error) => format!("{{\"error\":{}}}", quote(&error)),
    };
    std::fs::write(path, report).expect("write GPU diagnostics");
}
unsafe fn measure(ctx: &egui::Context) -> Result<String, String> {
    let owner = shared(ctx).ok_or("Native media renderer unavailable")?;
    let gl = &owner.gl;
    let mut painter =
        eframe::egui_glow::Painter::new(gl.clone(), "", None, false).map_err(|e| e.to_string())?;
    let fbo = gl.create_framebuffer()?;
    let target = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(target));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        1920,
        1080,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        None,
    );
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(target),
        0,
    );
    assert_eq!(
        gl.check_framebuffer_status(glow::FRAMEBUFFER),
        glow::FRAMEBUFFER_COMPLETE
    );
    gl.disable(glow::FRAMEBUFFER_SRGB);
    let mut rows = vec![];
    let mut checks = vec![];
    for size in [[960, 540], [1920, 1080], [3840, 2160]] {
        let frame = yuv_fixture(size);
        let rgb = rgb_of(&frame);
        let output = [size[0].min(1920), size[1].min(1080)];
        let mut native = None;
        let mut old: Option<Texture> = None;
        let mut old_times = vec![];
        let mut new_times = vec![];
        // Alternate order each iteration; each path includes CPU preparation, upload, draw and completion.
        for round in 0..55 {
            for legacy in if round % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                gl.finish();
                let start = Instant::now();
                if legacy {
                    let color = egui::ColorImage::from_rgb(size, &rgb);
                    if let Some(Texture::Managed(t)) = &mut old {
                        t.set(color, egui::TextureOptions::LINEAR);
                    } else {
                        old = Some(
                            ctx.load_texture("bench-video", color, egui::TextureOptions::LINEAR)
                                .into(),
                        );
                    }
                    flush_managed(ctx, &mut painter);
                    draw(gl, old.as_ref().unwrap(), &mut painter, output);
                } else {
                    video(ctx, &mut native, &frame);
                    draw(gl, native.as_ref().unwrap(), &mut painter, output);
                }
                gl.finish();
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if round >= 5 {
                    if legacy {
                        old_times.push(elapsed);
                    } else {
                        new_times.push(elapsed);
                    }
                }
            }
        }
        draw(gl, native.as_ref().unwrap(), &mut painter, output);
        let pixels = read_rgb(gl, output);
        for (x, y) in [
            (output[0] / 4, output[1] / 4),
            (output[0] * 3 / 4, output[1] / 4),
            (output[0] / 4, output[1] * 3 / 4),
            (output[0] * 3 / 4, output[1] * 3 / 4),
        ] {
            let source = (y * size[1] / output[1] * size[0] + x * size[0] / output[0]) * 3;
            for c in 0..3 {
                assert!(
                    pixels[(y * output[0] + x) * 3 + c].abs_diff(rgb[source + c]) <= 2,
                    "GPU YUV color/orientation mismatch"
                );
            }
        }
        let live = owner.state.lock().unwrap().live.len();
        assert_eq!(live, 3, "Playback must reuse exactly three plane textures");
        checks.push(format!(
            "\"YUV colors/orientation/reuse {}x{}\"",
            size[0], size[1]
        ));
        rows.push(format!("{{\"case\":\"video-upload-draw\",\"size\":{:?},\"legacy\":{},\"native\":{},\"legacyUploadBytes\":{},\"nativeUploadBytes\":{}}}",size,summary(old_times),summary(new_times),size[0]*size[1]*4,frame.pixels.len()));
        drop(native);
        drop(old);
        collect(ctx);
        flush_managed(ctx, &mut painter);
        assert_eq!(owner.state.lock().unwrap().live.len(), 0);
    }
    // Odd portrait planes and two independent live video textures.
    let odd = yuv_fixture([303, 539]);
    let mut first = None;
    let mut second = None;
    video(ctx, &mut first, &odd);
    video(ctx, &mut second, &yuv_fixture([539, 303]));
    draw(gl, first.as_ref().unwrap(), &mut painter, odd.size);
    let bytes = read_rgb(gl, odd.size);
    let rgb = rgb_of(&odd);
    for (x, y) in [(75, 135), (227, 135), (75, 403), (227, 403)] {
        for c in 0..3 {
            assert!(bytes[(y * 303 + x) * 3 + c].abs_diff(rgb[(y * 303 + x) * 3 + c]) <= 2);
        }
    }
    assert_eq!(owner.state.lock().unwrap().live.len(), 6);
    drop(first);
    drop(second);
    collect(ctx);
    assert_eq!(owner.state.lock().unwrap().live.len(), 0);
    checks.push("\"Odd portrait planes, independent previews and texture cleanup\"".into());
    let size = [6000, 4000];
    let photo = Photo {
        size,
        bytes: (0..size[0] * size[1] * 4)
            .map(|i| if i % 4 == 3 { 255 } else { (i % 251) as u8 })
            .collect(),
        alpha: false,
    };
    let mut old_times = vec![];
    let mut new_times = vec![];
    for round in 0..15 {
        for legacy in if round % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            gl.finish();
            let start = Instant::now();
            let texture = if legacy {
                let texture = ctx
                    .load_texture(
                        "bench-photo",
                        photo.color_image(),
                        egui::TextureOptions::LINEAR,
                    )
                    .into();
                flush_managed(ctx, &mut painter);
                texture
            } else {
                super::photo(ctx, &photo)
            };
            draw(gl, &texture, &mut painter, [1920, 1080]);
            gl.finish();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if round >= 3 {
                if legacy {
                    old_times.push(elapsed);
                } else {
                    new_times.push(elapsed);
                }
            }
            drop(texture);
            collect(ctx);
            flush_managed(ctx, &mut painter);
        }
    }
    rows.push(format!("{{\"case\":\"photo-upload-draw\",\"size\":{:?},\"legacy\":{},\"nativeWithMips\":{},\"nativeTextureBudgetBytes\":{}}}",size,summary(old_times),summary(new_times),mip_bytes(size)));
    let mut upload_times = vec![];
    let mut mip_times = vec![];
    let mut draw_times = vec![];
    for _ in 0..6 {
        gl.finish();
        let start = Instant::now();
        let texture = allocation(&owner, size, Kind::Photo)?;
        upload(&owner, &texture, &photo.bytes, true);
        gl.finish();
        upload_times.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        mipmaps(gl, texture.planes[0], size)?;
        gl.finish();
        mip_times.push(start.elapsed().as_secs_f64() * 1000.0);
        let texture = Texture::Native(texture);
        let start = Instant::now();
        draw(gl, &texture, &mut painter, [1920, 1080]);
        gl.finish();
        draw_times.push(start.elapsed().as_secs_f64() * 1000.0);
        drop(texture);
        collect(ctx);
    }
    rows.push(format!(
        "{{\"case\":\"photo-stages\",\"upload\":{},\"mips\":{},\"draw\":{}}}",
        summary(upload_times),
        summary(mip_times),
        summary(draw_times)
    ));
    let texture = super::photo(ctx, &photo);
    if let Texture::Native(t) = &texture {
        gl.bind_texture(glow::TEXTURE_2D, Some(t.planes[0]));
        assert_eq!(
            gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER),
            glow::LINEAR_MIPMAP_LINEAR as i32
        );
        for level in [1, 12] {
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(t.planes[0]),
                level,
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "Missing mip level {level}"
            );
        }
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(target),
            0,
        );
    } else {
        return Err("Expected native RGB photo".into());
    }
    drop(texture);
    collect(ctx);
    checks.push("\"Photo mip levels allocated on GPU\"".into());
    let transparent = Photo::from_image(image::DynamicImage::new_rgba8(16, 16));
    assert!(matches!(
        super::photo(ctx, &transparent),
        Texture::Managed(_)
    ));
    flush_managed(ctx, &mut painter);
    checks.push("\"Transparent photos keep premultiplied alpha path\"".into());
    // Board rotation must preserve source orientation on a non-square viewport.
    let colors = [
        [240, 40, 20, 255],
        [20, 210, 60, 255],
        [30, 80, 230, 255],
        [230, 190, 25, 255],
    ];
    let test_photo = Photo {
        size: [32, 16],
        alpha: false,
        bytes: (0..16)
            .flat_map(|y| (0..32).flat_map(move |x| colors[(x / 16) + (y / 8) * 2]))
            .collect(),
    };
    let rotated = super::photo(ctx, &test_photo);
    let Texture::Native(t) = &rotated else {
        return Err("Expected native board texture".into());
    };
    let viewport = egui::Rect::from_min_size(egui::pos2(71.0, 39.0), egui::vec2(400.0, 240.0));
    let rect = egui::Rect::from_center_size(
        viewport.center() + egui::vec2(17.0, -11.0),
        egui::vec2(160.0, 80.0),
    );
    for angle in [0.0, std::f32::consts::FRAC_PI_2, -0.47] {
        gl.viewport(0, 0, 400, 240);
        gl.disable(glow::SCISSOR_TEST);
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        owner
            .state
            .lock()
            .unwrap()
            .program
            .as_ref()
            .unwrap()
            .paint_rotated(gl, t, rect, viewport, angle);
        let pixels = read_rgb(gl, [400, 240]);
        for (index, point) in [
            egui::vec2(-40.0, -20.0),
            egui::vec2(40.0, -20.0),
            egui::vec2(-40.0, 20.0),
            egui::vec2(40.0, 20.0),
        ]
        .into_iter()
        .enumerate()
        {
            let point = rect.center() + crate::board::rotate(point, angle) - viewport.min;
            let offset = (point.y as usize * 400 + point.x as usize) * 3;
            for c in 0..3 {
                assert!(
                    pixels[offset + c].abs_diff(colors[index][c]) <= 2,
                    "Board rotation/color mismatch: {angle}"
                );
            }
        }
        assert_eq!(
            &pixels[..3],
            &[0, 0, 0],
            "Board texture escaped its rectangle"
        );
    }
    drop(rotated);
    collect(ctx);
    checks.push("\"Board GPU rotation at 0, 90 and arbitrary degrees\"".into());
    check_board_effects(ctx, gl, &mut painter);
    checks.push("\"Canvas crop, both flips, grayscale and opacity: native RGB, transparent managed RGBA and YUV, through egui callbacks\"".into());
    assert_eq!(gl.get_error(), glow::NO_ERROR, "OpenGL diagnostic error");
    let gpu = gl.get_parameter_string(glow::RENDERER);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    gl.delete_framebuffer(fbo);
    gl.delete_texture(target);
    painter.destroy();
    Ok(format!("{{\"gpu\":{},\"checks\":[{}],\"benchmarks\":[{}],\"note\":\"Release; alternating order; glFinish after upload+draw; excludes decode, scheduling, vsync and monitor latency. 1080p/4K video inputs are synthetic future-workload tests; app playback remains approximately 960x540.\"}}",quote(&gpu),checks.join(","),rows.join(",")))
}

/// Exercise the same egui callback path as the canvas, including sRGB managed textures.
unsafe fn check_board_effects(
    ctx: &egui::Context,
    gl: &glow::Context,
    painter: &mut eframe::egui_glow::Painter,
) {
    let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
    for kind in 0..3 {
        let colors = [
            [230, 40, 20, 255],
            [30, 210, 80, 160],
            [40, 80, 230, 90],
            [220, 160, 40, 0],
        ];
        let photo = Photo {
            size: [32, 32],
            alpha: kind == 1,
            bytes: (0..32)
                .flat_map(|y| {
                    (0..32).flat_map(move |x| {
                        let mut c = colors[x / 16 + (y / 16) * 2];
                        if kind != 1 {
                            c[3] = 255;
                        }
                        c
                    })
                })
                .collect(),
        };
        let frame = yuv_fixture([32, 32]);
        let texture = if kind == 2 {
            let mut t = None;
            video(ctx, &mut t, &frame);
            t.unwrap()
        } else {
            super::photo(ctx, &photo)
        };
        let expected = if kind == 2 {
            frame.color_image()
        } else {
            photo.color_image()
        };
        flush_managed(ctx, painter);
        for effects in [
            Effects::default(),
            Effects {
                uv: [1.0, 0.0, 0.0, 1.0],
                ..Default::default()
            },
            Effects {
                uv: [0.0, 1.0, 1.0, 0.0],
                ..Default::default()
            },
            Effects {
                uv: [0.05, 0.05, 0.45, 0.45],
                ..Default::default()
            },
            Effects {
                uv: [1.0, 1.0, 0.0, 0.0],
                opacity: 0.45,
                grayscale: true,
            },
        ] {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(area),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::background());
                    texture.paint_effects(&painter, area, area, 0.0, effects);
                },
            );
            for (id, delta) in output.textures_delta.set {
                painter.set_texture(id, &delta);
            }
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            painter.paint_primitives([64, 64], 1.0, &ctx.tessellate(output.shapes, 1.0));
            let pixels = read_rgb(gl, [64, 64]);
            for (x, y) in [(16, 16), (48, 16), (16, 48), (48, 48)] {
                let u = effects.uv[0] + (effects.uv[2] - effects.uv[0]) * (x as f32 + 0.5) / 64.0;
                let v = effects.uv[1] + (effects.uv[3] - effects.uv[1]) * (y as f32 + 0.5) / 64.0;
                let color =
                    expected.pixels[(v * 32.0) as usize * 32 + (u * 32.0) as usize].to_array();
                let gray =
                    color[0] as f32 * 0.2126 + color[1] as f32 * 0.7152 + color[2] as f32 * 0.0722;
                let alpha = color[3] as f32 / 255.0 * effects.opacity;
                for c in 0..3 {
                    let channel = if effects.grayscale {
                        gray
                    } else {
                        color[c] as f32
                    };
                    let wanted = (channel * effects.opacity + [20.0, 30.0, 40.0][c] * (1.0 - alpha))
                        .round() as u8;
                    let got = pixels[(y * 64 + x) * 3 + c];
                    assert!(wanted.abs_diff(got)<=3,"Canvas GPU effect mismatch kind={kind}, effects={effects:?}, point={x},{y}, channel={c}: {got} vs {wanted}");
                }
            }
        }
    }
}
