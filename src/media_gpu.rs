//! Native OpenGL media textures. All GL calls run on the UI/render thread.
//! Dropping a cached handle only queues deletion; it never calls GL from a worker.
use crate::video_frame::{Format, Frame};
use eframe::{
    egui,
    glow::{self, HasContext},
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, Weak},
};
mod diagnostic;
pub use diagnostic::run as run_diagnostics;

pub struct Photo {
    pub size: [usize; 2],
    pub bytes: Vec<u8>,
    pub alpha: bool,
}
impl Photo {
    pub fn from_image(image: image::DynamicImage) -> Self {
        let size = [image.width() as usize, image.height() as usize];
        let has_alpha = image.color().has_alpha();
        // RGBA is the driver's fast upload layout. Packed RGB costs an expensive driver conversion
        // on NVIDIA; perform the expansion once in the decode worker, without a second ColorImage.
        let bytes = image.into_rgba8().into_raw();
        let alpha = has_alpha && bytes.chunks_exact(4).any(|p| p[3] != 255);
        Self { size, bytes, alpha }
    }
    fn color_image(&self) -> egui::ColorImage {
        egui::ColorImage::from_rgba_unmultiplied(self.size, &self.bytes)
    }
    pub fn gpu_bytes(&self, ctx: &egui::Context) -> usize {
        if !self.alpha && shared(ctx).is_some_and(|s| s.immutable_textures) {
            mip_bytes(self.size)
        } else {
            self.size[0] * self.size[1] * 4
        }
    }
}
pub fn mip_bytes([mut w, mut h]: [usize; 2]) -> usize {
    let mut bytes = 0;
    loop {
        bytes += w * h * 4; // RGB8 may occupy four bytes on the driver; budget conservatively.
        if w <= 1 && h <= 1 {
            return bytes;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

#[derive(Clone)]
pub enum Texture {
    Managed(egui::TextureHandle),
    Native(Arc<Allocation>),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    pub uv: [f32; 4],
    pub opacity: f32,
    pub grayscale: bool,
}
impl Default for Effects {
    fn default() -> Self {
        Self {
            uv: [0.0, 0.0, 1.0, 1.0],
            opacity: 1.0,
            grayscale: false,
        }
    }
}
impl From<egui::TextureHandle> for Texture {
    fn from(t: egui::TextureHandle) -> Self {
        Self::Managed(t)
    }
}
impl Texture {
    pub fn size(&self) -> [usize; 2] {
        match self {
            Self::Managed(t) => t.size(),
            Self::Native(t) => t.size,
        }
    }
    #[cfg(test)]
    pub fn id(&self) -> egui::TextureId {
        match self {
            Self::Managed(t) => t.id(),
            Self::Native(t) => egui::TextureId::User(Arc::as_ptr(t) as u64),
        }
    }
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect, viewport: egui::Rect) {
        self.paint_rotated(painter, rect, viewport, 0.0);
    }
    pub fn paint_rotated(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        viewport: egui::Rect,
        angle: f32,
    ) {
        self.paint_effects(painter, rect, viewport, angle, Effects::default());
    }
    pub fn paint_effects(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        viewport: egui::Rect,
        angle: f32,
        effects: Effects,
    ) {
        match self {
            Self::Managed(t) => {
                if effects.grayscale {
                    if let Some(shared) = shared(painter.ctx()) {
                        let handle = t.clone();
                        painter.add(egui::PaintCallback {
                            rect: viewport,
                            callback: Arc::new(eframe::egui_glow::CallbackFn::new(
                                move |_, painter| {
                                    if let Some(texture) = painter.texture(handle.id()) {
                                        let state = shared.state.lock().unwrap();
                                        if let Some(program) = &state.program {
                                            unsafe {
                                                program.paint_effects(
                                                    &shared.gl,
                                                    &[texture],
                                                    false,
                                                    program.managed_srgb,
                                                    rect,
                                                    viewport,
                                                    angle,
                                                    effects,
                                                );
                                            }
                                        }
                                    }
                                },
                            )),
                        });
                        return;
                    }
                }
                let mut mesh = egui::Mesh::with_texture(t.id());
                let [u0, v0, u1, v1] = effects.uv;
                for (p, uv) in [
                    (rect.left_top(), egui::pos2(u0, v0)),
                    (rect.right_top(), egui::pos2(u1, v0)),
                    (rect.left_bottom(), egui::pos2(u0, v1)),
                    (rect.right_bottom(), egui::pos2(u1, v1)),
                ] {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: rect.center() + crate::board::rotate(p - rect.center(), angle),
                        uv,
                        color: egui::Color32::WHITE.gamma_multiply(effects.opacity),
                    });
                }
                mesh.indices = vec![0, 1, 2, 2, 1, 3];
                painter.add(mesh);
            }
            Self::Native(texture) => {
                let texture = texture.clone();
                painter.add(egui::PaintCallback {
                    rect: viewport,
                    callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, _| {
                        if let Some(shared) = texture.owner.upgrade() {
                            let state = shared.state.lock().unwrap();
                            if let Some(program) = &state.program {
                                unsafe {
                                    program.paint_effects(
                                        &shared.gl,
                                        &texture.planes,
                                        texture.kind == Kind::Video(Format::Yuv420),
                                        false,
                                        rect,
                                        viewport,
                                        angle,
                                        effects,
                                    );
                                }
                            }
                        }
                    })),
                });
            }
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Photo,
    Video(Format),
}
pub struct Allocation {
    size: [usize; 2],
    kind: Kind,
    planes: Vec<glow::Texture>,
    owner: Weak<Shared>,
}
impl Drop for Allocation {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner
                .state
                .lock()
                .unwrap()
                .retired
                .extend(self.planes.iter().copied());
        }
    }
}
struct State {
    program: Option<Program>,
    live: HashSet<glow::Texture>,
    retired: Vec<glow::Texture>,
}
struct Shared {
    gl: Arc<glow::Context>,
    state: Mutex<State>,
    immutable_textures: bool,
}
fn key() -> egui::Id {
    egui::Id::new("native-media-renderer")
}
fn shared(ctx: &egui::Context) -> Option<Arc<Shared>> {
    ctx.data(|d| d.get_temp::<Arc<Shared>>(key()))
}

pub fn install(ctx: &egui::Context, gl: Option<&Arc<glow::Context>>) {
    if std::env::var_os("VIEW_CALIPERS_MEDIA_LEGACY").is_some() {
        return;
    }
    if let Some(gl) = gl {
        let v = gl.version();
        let immutable_textures = (v.is_embedded && v.major >= 3)
            || v.major > 4
            || (v.major == 4 && v.minor >= 2)
            || gl.supported_extensions().contains("GL_ARB_texture_storage");
        match unsafe { Program::new(gl) } {
            Ok(program) => ctx.data_mut(|d| {
                d.insert_temp(
                    key(),
                    Arc::new(Shared {
                        gl: gl.clone(),
                        immutable_textures,
                        state: Mutex::new(State {
                            program: Some(program),
                            live: HashSet::new(),
                            retired: vec![],
                        }),
                    }),
                )
            }),
            Err(e) => {
                eprintln!("Native media renderer unavailable; using egui texture upload: {e}")
            }
        }
    }
}
pub fn collect(ctx: &egui::Context) {
    if let Some(shared) = shared(ctx) {
        let mut state = shared.state.lock().unwrap();
        for texture in std::mem::take(&mut state.retired) {
            if state.live.remove(&texture) {
                unsafe {
                    shared.gl.delete_texture(texture);
                }
            }
        }
    }
}
pub fn shutdown(ctx: &egui::Context) {
    if let Some(shared) = shared(ctx) {
        let mut state = shared.state.lock().unwrap();
        unsafe {
            for texture in state.live.drain() {
                shared.gl.delete_texture(texture);
            }
            if let Some(program) = state.program.take() {
                program.destroy(&shared.gl);
            }
        }
        state.retired.clear();
    }
}
unsafe fn allocation(
    shared: &Arc<Shared>,
    size: [usize; 2],
    kind: Kind,
) -> Result<Arc<Allocation>, String> {
    let gl = &shared.gl;
    let count = if kind == Kind::Video(Format::Yuv420) {
        3
    } else {
        1
    };
    let mut planes = vec![];
    for _ in 0..count {
        match gl.create_texture() {
            Ok(texture) => planes.push(texture),
            Err(error) => {
                for texture in planes {
                    gl.delete_texture(texture);
                }
                return Err(error);
            }
        }
    }
    shared
        .state
        .lock()
        .unwrap()
        .live
        .extend(planes.iter().copied());
    Ok(Arc::new(Allocation {
        size,
        kind,
        planes,
        owner: Arc::downgrade(shared),
    }))
}
unsafe fn upload(shared: &Shared, allocation: &Allocation, bytes: &[u8], new: bool) {
    let gl = &shared.gl;
    let alignment = gl.get_parameter_i32(glow::UNPACK_ALIGNMENT);
    let row_length = gl.get_parameter_i32(glow::UNPACK_ROW_LENGTH);
    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
    gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
    gl.active_texture(glow::TEXTURE0);
    let [w, h] = allocation.size;
    let planar = allocation.kind == Kind::Video(Format::Yuv420);
    let mut offset = 0;
    for (i, texture) in allocation.planes.iter().enumerate() {
        let [pw, ph] = if i == 0 {
            [w, h]
        } else {
            [w.div_ceil(2), h.div_ceil(2)]
        };
        let photo = allocation.kind == Kind::Photo;
        let format = if planar {
            glow::RED
        } else if photo {
            glow::RGBA
        } else {
            glow::RGB
        };
        let internal = if planar {
            glow::R8
        } else if photo {
            glow::RGBA8
        } else {
            glow::RGB8
        };
        let length = pw
            * ph
            * if planar {
                1
            } else if photo {
                4
            } else {
                3
            };
        gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
        if new {
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                if allocation.kind == Kind::Photo {
                    glow::LINEAR_MIPMAP_LINEAR
                } else {
                    glow::LINEAR
                } as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            for parameter in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::CLAMP_TO_EDGE as i32);
            }
            if photo {
                // Reserve the entire pyramid once. Defining levels after uploading level zero
                // makes some drivers repeatedly reallocate/copy the large base image.
                let levels = pw.max(ph).ilog2() as i32 + 1;
                gl.tex_storage_2d(glow::TEXTURE_2D, levels, glow::RGBA8, pw as i32, ph as i32);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    pw as i32,
                    ph as i32,
                    format,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(&bytes[offset..offset + length]),
                );
            } else {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    internal as i32,
                    pw as i32,
                    ph as i32,
                    0,
                    format,
                    glow::UNSIGNED_BYTE,
                    Some(&bytes[offset..offset + length]),
                );
            }
        } else {
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                pw as i32,
                ph as i32,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(&bytes[offset..offset + length]),
            );
        }
        offset += length;
    }
    gl.bind_texture(glow::TEXTURE_2D, None);
    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, alignment);
    gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, row_length);
}
pub fn photo(ctx: &egui::Context, photo: &Photo) -> Texture {
    // Preserve egui's premultiplied-alpha filtering for transparent images.
    if !photo.alpha {
        if let Some(shared) = shared(ctx).filter(|s| s.immutable_textures) {
            if let Ok(texture) = unsafe { allocation(&shared, photo.size, Kind::Photo) } {
                unsafe {
                    upload(&shared, &texture, &photo.bytes, true);
                    if let Err(error) = mipmaps(&shared.gl, texture.planes[0], photo.size) {
                        eprintln!("Photo mipmap fallback: {error}");
                        shared
                            .gl
                            .bind_texture(glow::TEXTURE_2D, Some(texture.planes[0]));
                        shared.gl.generate_mipmap(glow::TEXTURE_2D);
                        shared.gl.bind_texture(glow::TEXTURE_2D, None);
                    }
                }
                return Texture::Native(texture);
            }
        }
    }
    ctx.load_texture(
        "photo-preview",
        photo.color_image(),
        egui::TextureOptions::LINEAR,
    )
    .into()
}
/// Storage for the complete pyramid is reserved before upload. That avoids expensive
/// driver reallocations when generating levels after a large mutable level-zero image.
unsafe fn mipmaps(
    gl: &glow::Context,
    texture: glow::Texture,
    _size: [usize; 2],
) -> Result<(), String> {
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    gl.generate_mipmap(glow::TEXTURE_2D);
    gl.bind_texture(glow::TEXTURE_2D, None);
    Ok(())
}
pub fn video(ctx: &egui::Context, target: &mut Option<Texture>, frame: &Frame) {
    if let Some(shared) = shared(ctx) {
        let reusable = target.as_ref().and_then(|texture| match texture {
            Texture::Native(t) if t.size == frame.size && t.kind == Kind::Video(frame.format) => {
                Some(t.clone())
            }
            _ => None,
        });
        let new = reusable.is_none();
        if let Ok(texture) = reusable.map(Ok).unwrap_or_else(|| unsafe {
            allocation(&shared, frame.size, Kind::Video(frame.format))
        }) {
            unsafe {
                upload(&shared, &texture, &frame.pixels, new);
            }
            *target = Some(Texture::Native(texture));
            return;
        }
    }
    let color = frame.color_image();
    if let Some(Texture::Managed(texture)) = target {
        texture.set(color, egui::TextureOptions::LINEAR);
    } else {
        *target = Some(
            ctx.load_texture("video-frame", color, egui::TextureOptions::LINEAR)
                .into(),
        );
    }
}

struct Program {
    program: glow::Program,
    vao: glow::VertexArray,
    axis_x: Option<glow::UniformLocation>,
    axis_y: Option<glow::UniformLocation>,
    offset: Option<glow::UniformLocation>,
    planar: Option<glow::UniformLocation>,
    uv_rect: Option<glow::UniformLocation>,
    opacity: Option<glow::UniformLocation>,
    grayscale: Option<glow::UniformLocation>,
    srgb: Option<glow::UniformLocation>,
    managed_srgb: bool,
    samplers: [Option<glow::UniformLocation>; 3],
}
impl Program {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let header = if gl.version().is_embedded {
            "#version 300 es\nprecision highp float;\n"
        } else {
            "#version 330 core\n"
        };
        let vertex = "uniform vec2 axis_x; uniform vec2 axis_y; uniform vec2 offset; out vec2 uv;
void main() { vec2 p = vec2(float(gl_VertexID & 1),float((gl_VertexID >> 1) & 1)); uv=p;
vec2 d=p*2.0-1.0; gl_Position=vec4(d.x*axis_x+d.y*axis_y+offset,0,1); }";
        let fragment = "in vec2 uv; out vec4 color; uniform sampler2D t0; uniform sampler2D t1; uniform sampler2D t2; uniform int planar;
uniform vec4 uv_rect; uniform float opacity; uniform int grayscale; uniform int srgb;
vec3 gamma_from_linear(vec3 rgb) {
    return mix(1.055*pow(rgb,vec3(1.0/2.4))-0.055,rgb*12.92,vec3(lessThan(rgb,vec3(0.0031308))));
}
void main() { vec2 sample_uv=mix(uv_rect.xy,uv_rect.zw,uv);
if(planar==0) {color=texture(t0,sample_uv);} else {
float y=(texture(t0,sample_uv).r*255.0-16.0)/219.0;
float u=(texture(t1,sample_uv).r*255.0-128.0)/224.0; float v=(texture(t2,sample_uv).r*255.0-128.0)/224.0;
color=vec4(clamp(vec3(y+1.5748*v,y-0.187324*u-0.468124*v,y+1.8556*u),0.0,1.0),1); }
if(srgb!=0) {color.rgb=gamma_from_linear(color.rgb);}
if(grayscale!=0) {color.rgb=vec3(dot(color.rgb,vec3(0.2126,0.7152,0.0722)));} color*=opacity; }";
        let program = gl.create_program()?;
        for (kind, source) in [
            (glow::VERTEX_SHADER, vertex),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = match gl.create_shader(kind) {
                Ok(s) => s,
                Err(e) => {
                    gl.delete_program(program);
                    return Err(e);
                }
            };
            gl.shader_source(shader, &format!("{header}{source}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let error = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(error);
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(error);
        }
        let vao = match gl.create_vertex_array() {
            Ok(v) => v,
            Err(e) => {
                gl.delete_program(program);
                return Err(e);
            }
        };
        Ok(Self {
            program,
            vao,
            axis_x: gl.get_uniform_location(program, "axis_x"),
            axis_y: gl.get_uniform_location(program, "axis_y"),
            offset: gl.get_uniform_location(program, "offset"),
            planar: gl.get_uniform_location(program, "planar"),
            uv_rect: gl.get_uniform_location(program, "uv_rect"),
            opacity: gl.get_uniform_location(program, "opacity"),
            grayscale: gl.get_uniform_location(program, "grayscale"),
            srgb: gl.get_uniform_location(program, "srgb"),
            managed_srgb: gl.version().is_embedded
                || gl.supported_extensions().iter().any(|e| e.contains("sRGB")),
            samplers: ["t0", "t1", "t2"].map(|n| gl.get_uniform_location(program, n)),
        })
    }
    unsafe fn paint(
        &self,
        gl: &glow::Context,
        texture: &Allocation,
        rect: egui::Rect,
        viewport: egui::Rect,
    ) {
        self.paint_rotated(gl, texture, rect, viewport, 0.0);
    }
    unsafe fn paint_rotated(
        &self,
        gl: &glow::Context,
        texture: &Allocation,
        rect: egui::Rect,
        viewport: egui::Rect,
        angle: f32,
    ) {
        self.paint_effects(
            gl,
            &texture.planes,
            texture.kind == Kind::Video(Format::Yuv420),
            false,
            rect,
            viewport,
            angle,
            Effects::default(),
        );
    }
    unsafe fn paint_effects(
        &self,
        gl: &glow::Context,
        planes: &[glow::Texture],
        planar: bool,
        srgb: bool,
        rect: egui::Rect,
        viewport: egui::Rect,
        angle: f32,
        effects: Effects,
    ) {
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.enable(glow::BLEND);
        gl.blend_equation(glow::FUNC_ADD);
        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        let (sin, cos) = angle.sin_cos();
        gl.uniform_2_f32(
            self.axis_x.as_ref(),
            cos * rect.width() / viewport.width(),
            -sin * rect.width() / viewport.height(),
        );
        gl.uniform_2_f32(
            self.axis_y.as_ref(),
            -sin * rect.height() / viewport.width(),
            -cos * rect.height() / viewport.height(),
        );
        let delta = rect.center() - viewport.center();
        gl.uniform_2_f32(
            self.offset.as_ref(),
            2.0 * delta.x / viewport.width(),
            -2.0 * delta.y / viewport.height(),
        );
        gl.uniform_1_i32(self.planar.as_ref(), i32::from(planar));
        let [u0, v0, u1, v1] = effects.uv;
        gl.uniform_4_f32(self.uv_rect.as_ref(), u0, v0, u1, v1);
        gl.uniform_1_f32(self.opacity.as_ref(), effects.opacity);
        gl.uniform_1_i32(self.grayscale.as_ref(), i32::from(effects.grayscale));
        gl.uniform_1_i32(self.srgb.as_ref(), i32::from(srgb));
        for (i, plane) in planes.iter().enumerate() {
            gl.active_texture(glow::TEXTURE0 + i as u32);
            gl.bind_texture(glow::TEXTURE_2D, Some(*plane));
            gl.uniform_1_i32(self.samplers[i].as_ref(), i as i32);
        }
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        for i in 0..planes.len() {
            gl.active_texture(glow::TEXTURE0 + i as u32);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_vertex_array(None);
        gl.use_program(None);
    }
    unsafe fn destroy(self, gl: &glow::Context) {
        gl.delete_vertex_array(self.vao);
        gl.delete_program(self.program);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn photo_payload_and_mip_budget_include_nonsquare_levels() {
        assert_eq!(mip_bytes([4, 2]), 44);
        assert_eq!(mip_bytes([1, 8]), 60);
        let rgb = Photo::from_image(image::DynamicImage::new_rgb8(3, 5));
        assert_eq!(rgb.bytes.len(), 60);
        assert!(!rgb.alpha);
        let rgba = Photo::from_image(image::DynamicImage::new_rgba8(3, 5));
        assert_eq!(rgba.bytes.len(), 60);
        assert!(rgba.alpha);
        let opaque_png = Photo::from_image(image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255])),
        ));
        assert!(
            !opaque_png.alpha,
            "Opaque RGBA images can use the native path"
        );
    }
}
