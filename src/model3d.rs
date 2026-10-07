use std::{path::{Path, PathBuf}, sync::{Arc, Mutex, Condvar, atomic::{AtomicBool, Ordering}}, hash::{Hash, Hasher}};
use anyhow::{anyhow, Context, Result};
use eframe::{egui, glow::{self, HasContext}};

pub const EXTENSIONS: &[&str] = &["obj", "gltf", "glb", "fbx", "usd", "usda", "usdc", "usdz"];
const MAX_TRIANGLES: usize = 2_000_000;

pub struct Model { pub vertices: Vec<f32>, pub triangles: usize }
impl Model {
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 12 || &bytes[..8] != b"AV3D0001" { return Err(anyhow!("Invalid 3D preview data")); }
        let triangles = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
        if triangles == 0 || triangles > MAX_TRIANGLES || bytes.len() != 12 + triangles * 108 { return Err(anyhow!("Invalid or oversized 3D mesh")); }
        let mut vertices: Vec<f32> = bytes[12..].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        if vertices.iter().any(|v| !v.is_finite()) { return Err(anyhow!("The model contains invalid coordinates")); }
        let mut min = [f32::INFINITY; 3]; let mut max = [f32::NEG_INFINITY; 3];
        for vertex in vertices.chunks_exact(9) { for i in 0..3 { min[i] = min[i].min(vertex[i]); max[i] = max[i].max(vertex[i]); } }
        let center = std::array::from_fn::<_, 3, _>(|i| (min[i] + max[i]) * 0.5);
        let radius = vertices.chunks_exact(9).map(|v| ((v[0]-center[0]).powi(2)+(v[1]-center[1]).powi(2)+(v[2]-center[2]).powi(2)).sqrt()).fold(0.0_f32, f32::max).max(0.000001);
        for vertex in vertices.chunks_exact_mut(9) { for i in 0..3 { vertex[i] = (vertex[i] - center[i]) / radius; } }
        Ok(Self { vertices, triangles })
    }
}

pub fn blender() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("BLENDER_PATH").map(PathBuf::from).filter(|p| p.is_file()) { return Ok(path); }
    let base = PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into())).join("Blender Foundation");
    let mut candidates: Vec<_> = std::fs::read_dir(base).into_iter().flatten().flatten().map(|entry| entry.path().join("blender.exe")).filter(|p| p.is_file()).collect();
    candidates.sort();
    candidates.pop().ok_or_else(|| anyhow!("Blender is required for 3D previews. Install Blender or set BLENDER_PATH to blender.exe."))
}

pub fn load(path: &Path) -> Result<Arc<Model>> {
    // Serialize imports to avoid launching multiple heavyweight importers for the same gallery.
    static IMPORT: Mutex<()> = Mutex::new(());
    let _guard = IMPORT.lock().unwrap();
    let path = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&path)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    "solid-model-v1".hash(&mut hash); path.hash(&mut hash); metadata.len().hash(&mut hash); metadata.modified()?.hash(&mut hash);
    let root = crate::thumbnail_cache::directory(); std::fs::create_dir_all(&root)?;
    let cached = root.join(format!("model-{:016x}.bin", hash.finish()));
    if let Ok(bytes) = std::fs::read(&cached) { if let Ok(model) = Model::from_bytes(&bytes) { return Ok(Arc::new(model)); } }
    let work = std::env::temp_dir().join(format!("archview-3d-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
    std::fs::create_dir_all(&work)?;
    struct Cleanup(PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let _cleanup = Cleanup(work.clone());
    let script = work.join("import.py"); let output = work.join("mesh.bin");
    std::fs::write(&script, include_str!("import_model.py"))?;
    let mut command = std::process::Command::new(blender()?);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    let result = command.args(["--background", "--factory-startup", "--disable-autoexec", "--python-exit-code", "1", "--python"]).arg(script)
        .arg("--").arg(&path).arg(&output).stdin(std::process::Stdio::null()).output().context("Could not start Blender importer")?;
    if !result.status.success() {
        let log = format!("{}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        return Err(anyhow!("3D import failed: {}", log.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")));
    }
    let bytes = std::fs::read(&output).context("The importer did not produce a mesh")?;
    let model = Model::from_bytes(&bytes)?;
    let temporary = cached.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&temporary, &bytes).is_ok() { let _ = std::fs::rename(&temporary, &cached); let _ = std::fs::remove_file(&temporary); }
    Ok(Arc::new(model))
}

pub struct Loader {
    pending: Arc<(Mutex<Option<(u64, PathBuf)>>, Condvar)>, stopped: Arc<AtomicBool>,
    results: std::sync::mpsc::Receiver<(u64, Result<Arc<Model>>)>, next: u64,
}
impl Loader {
    pub fn new(ctx: egui::Context) -> Self {
        let pending = Arc::new((Mutex::new(None::<(u64, PathBuf)>), Condvar::new())); let stopped = Arc::new(AtomicBool::new(false));
        let queue = pending.clone(); let stop = stopped.clone(); let (tx, results) = std::sync::mpsc::channel();
        std::thread::spawn(move || loop {
            let mut request = queue.0.lock().unwrap();
            while request.is_none() && !stop.load(Ordering::Relaxed) { request = queue.1.wait(request).unwrap(); }
            if stop.load(Ordering::Relaxed) { break; }
            let (id, path) = request.take().unwrap(); drop(request);
            let result = load(&path);
            if stop.load(Ordering::Relaxed) || tx.send((id, result)).is_err() { break; } ctx.request_repaint();
        });
        Self { pending, stopped, results, next: 0 }
    }
    pub fn request(&mut self, path: PathBuf) -> u64 { self.next += 1; *self.pending.0.lock().unwrap() = Some((self.next, path)); self.pending.1.notify_one(); self.next }
    pub fn poll(&self) -> Option<(u64, Result<Arc<Model>>)> { self.results.try_iter().last() }
}
impl Drop for Loader { fn drop(&mut self) { let _guard = self.pending.0.lock().unwrap(); self.stopped.store(true, Ordering::Relaxed); self.pending.1.notify_one(); } }

#[derive(Clone, Copy)]
pub struct View { pub yaw: f32, pub pitch: f32, pub zoom: f32, pub pan: [f32; 2], pub wireframe: bool }
impl Default for View { fn default() -> Self { Self { yaw: 0.65, pitch: -0.35, zoom: 0.82, pan: [0.0; 2], wireframe: false } } }
fn rotate(v: [f32; 3], view: View) -> [f32; 3] {
    let (s, c) = view.yaw.sin_cos(); let x = c*v[0]+s*v[2]; let z = -s*v[0]+c*v[2];
    let (s, c) = view.pitch.sin_cos(); [x, c*v[1]-s*z, s*v[1]+c*z]
}

pub fn thumbnail(model: &Model) -> egui::ColorImage {
    let (w, h) = (320_usize, 240_usize); let view = View::default();
    let mut image = egui::ColorImage::new([w,h], egui::Color32::from_rgb(29,32,38)); let mut depths = vec![f32::INFINITY; w*h];
    for face in model.vertices.chunks_exact(27) {
        let mut points = [[0.0;3];3];
        for i in 0..3 { let p = rotate([face[i*9],face[i*9+1],face[i*9+2]],view); points[i]=[w as f32*0.5+p[0]*95.0,h as f32*0.5-p[1]*95.0,p[2]]; }
        let [a,b,c] = points; let area = (b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0]); if area.abs()<0.00001 {continue;}
        let minx=a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize; let maxx=a[0].max(b[0]).max(c[0]).ceil().min((w-1) as f32).max(0.0) as usize;
        let miny=a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize; let maxy=a[1].max(b[1]).max(c[1]).ceil().min((h-1) as f32).max(0.0) as usize;
        let n=rotate([face[3],face[4],face[5]],view); let light=0.3+0.7*(n[0]*0.3+n[1]*0.5+n[2]*0.8).abs().min(1.0);
        let color=egui::Color32::from_rgb((face[6].clamp(0.0,1.0)*light*255.0) as u8,(face[7].clamp(0.0,1.0)*light*255.0) as u8,(face[8].clamp(0.0,1.0)*light*255.0) as u8);
        for y in miny..=maxy { for x in minx..=maxx {
            let (px,py)=(x as f32+0.5,y as f32+0.5);
            let u=((b[0]-px)*(c[1]-py)-(b[1]-py)*(c[0]-px))/area;
            let v=((c[0]-px)*(a[1]-py)-(c[1]-py)*(a[0]-px))/area; let t=1.0-u-v;
            if u>=0.0 && v>=0.0 && t>=0.0 { let depth=u*a[2]+v*b[2]+t*c[2]; let i=y*w+x; if depth<depths[i] {depths[i]=depth;image.pixels[i]=color;} }
        } }
    }
    image
}

pub struct Gpu { model: Arc<Model>, program: glow::Program, vao: glow::VertexArray, buffer: glow::Buffer }
impl Gpu {
    unsafe fn new(gl: &glow::Context, model: Arc<Model>) -> Result<Self> {
        let program=gl.create_program().map_err(|e|anyhow!(e))?;
        let vertex=r#"#version 330 core
layout(location=0) in vec3 pos; layout(location=1) in vec3 normal; layout(location=2) in vec3 color;
uniform vec2 angles; uniform vec2 scale; uniform vec2 pan; out vec3 col; out vec3 n;
vec3 turn(vec3 v) { float s=sin(angles.x), c=cos(angles.x); v=vec3(c*v.x+s*v.z,v.y,-s*v.x+c*v.z); s=sin(angles.y);c=cos(angles.y);return vec3(v.x,c*v.y-s*v.z,s*v.y+c*v.z); }
void main(){ vec3 p=turn(pos);gl_Position=vec4(p.xy*scale+pan,p.z*0.3,1.0); n=turn(normal);col=color; }
"#;
        let fragment=r#"#version 330 core
in vec3 col;in vec3 n;out vec4 output_color;
void main(){float light=0.3+0.7*abs(dot(normalize(n),normalize(vec3(0.3,0.5,0.8))));output_color=vec4(col*light,1.0);}
"#;
        for (kind, source) in [(glow::VERTEX_SHADER,vertex),(glow::FRAGMENT_SHADER,fragment)] {
            let shader=gl.create_shader(kind).map_err(|e|anyhow!(e))?;gl.shader_source(shader,source);gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {let error=gl.get_shader_info_log(shader);gl.delete_shader(shader);gl.delete_program(program);return Err(anyhow!(error));}
            gl.attach_shader(program,shader);gl.delete_shader(shader);
        }
        gl.link_program(program);if !gl.get_program_link_status(program) {let error=gl.get_program_info_log(program);gl.delete_program(program);return Err(anyhow!(error));}
        let vao=gl.create_vertex_array().map_err(|e|anyhow!(e))?;let buffer=gl.create_buffer().map_err(|e|anyhow!(e))?;
        gl.bind_vertex_array(Some(vao));gl.bind_buffer(glow::ARRAY_BUFFER,Some(buffer));
        let bytes=std::slice::from_raw_parts(model.vertices.as_ptr() as *const u8,model.vertices.len()*4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER,bytes,glow::STATIC_DRAW);
        for i in 0..3 {gl.enable_vertex_attrib_array(i);gl.vertex_attrib_pointer_f32(i,3,glow::FLOAT,false,36,(i*12) as i32);}
        gl.bind_vertex_array(None);gl.bind_buffer(glow::ARRAY_BUFFER,None);
        Ok(Self{model,program,vao,buffer})
    }
    pub unsafe fn destroy(&self,gl:&glow::Context){gl.delete_buffer(self.buffer);gl.delete_vertex_array(self.vao);gl.delete_program(self.program);}
    unsafe fn paint(&self,gl:&glow::Context,view:View,aspect:f32){
        gl.enable(glow::DEPTH_TEST);gl.depth_func(glow::LESS);gl.depth_mask(true);gl.disable(glow::CULL_FACE);gl.disable(glow::BLEND);
        gl.clear_color(0.11,0.12,0.15,1.0);gl.clear_depth_f32(1.0);gl.clear(glow::COLOR_BUFFER_BIT|glow::DEPTH_BUFFER_BIT);
        gl.use_program(Some(self.program));gl.bind_vertex_array(Some(self.vao));
        gl.uniform_2_f32(gl.get_uniform_location(self.program,"angles").as_ref(),view.yaw,view.pitch);
        gl.uniform_2_f32(gl.get_uniform_location(self.program,"scale").as_ref(),view.zoom/aspect.max(1.0),view.zoom*aspect.min(1.0));
        gl.uniform_2_f32(gl.get_uniform_location(self.program,"pan").as_ref(),view.pan[0],view.pan[1]);
        if view.wireframe {gl.polygon_mode(glow::FRONT_AND_BACK,glow::LINE);}
        gl.draw_arrays(glow::TRIANGLES,0,(self.model.triangles*3) as i32);
        gl.polygon_mode(glow::FRONT_AND_BACK,glow::FILL);gl.bind_vertex_array(None);gl.use_program(None);gl.disable(glow::DEPTH_TEST);
    }
}

pub type Renderer = Arc<Mutex<Option<Gpu>>>;
pub fn show(ui:&mut egui::Ui, model:Arc<Model>, view:&mut View, renderer:Renderer, settings:&crate::settings::Settings) -> bool {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Reset view").clicked(){*view=View::default();}
        ui.checkbox(&mut view.wireframe,"Wireframe");ui.label(format!("{} triangles · Solid materials",model.triangles));
    });
    let rect=ui.available_rect_before_wrap();let response=ui.allocate_rect(rect,egui::Sense::click_and_drag());
    if response.dragged() {
        let delta=ui.ctx().input(|i|i.pointer.delta());
        if ui.ctx().input(|i|i.modifiers.shift){view.pan[0]+=delta.x/rect.width()*2.0;view.pan[1]-=delta.y/rect.height()*2.0;}
        else {view.yaw+=delta.x*settings.orbit_speed*if settings.invert_x{-1.0}else{1.0};view.pitch=(view.pitch+delta.y*settings.orbit_speed*if settings.invert_y{-1.0}else{1.0}).clamp(-1.55,1.55);}
    }
    if response.hovered(){let scroll=ui.input(|i|i.raw_scroll_delta.y);view.zoom=(view.zoom*(scroll*settings.zoom_speed).exp()).clamp(0.05,20.0);}
    let view=*view;let aspect=rect.width()/rect.height().max(1.0);
    ui.painter().add(egui::PaintCallback{rect,callback:Arc::new(eframe::egui_glow::CallbackFn::new(move |info,painter|{
        let gl=painter.gl();let mut state=renderer.lock().unwrap();unsafe{
            if state.as_ref().is_none_or(|gpu|!Arc::ptr_eq(&gpu.model,&model)){
                if let Some(old)=state.take(){old.destroy(gl);}
                match Gpu::new(gl,model.clone()){Ok(gpu)=>*state=Some(gpu),Err(error)=>{eprintln!("3D GPU: {error}");return;}}
            }
            if let Some(gpu)=state.as_ref(){
                // glClear ignores the viewport: clip it to the 3D content, not the entire preview panel.
                let mut previous_scissor=[0;4];gl.get_parameter_i32_slice(glow::SCISSOR_BOX,&mut previous_scissor);
                let viewport=info.viewport_in_pixels();
                let left=viewport.left_px.max(previous_scissor[0]);let bottom=viewport.from_bottom_px.max(previous_scissor[1]);
                let right=(viewport.left_px+viewport.width_px).min(previous_scissor[0]+previous_scissor[2]);
                let top=(viewport.from_bottom_px+viewport.height_px).min(previous_scissor[1]+previous_scissor[3]);
                gl.scissor(left,bottom,(right-left).max(0),(top-bottom).max(0));
                gpu.paint(gl,view,aspect);
                // Optional diagnostic capture from the actual GPU framebuffer.
                static CAPTURED: AtomicBool = AtomicBool::new(false);
                if let Some(path)=std::env::var_os("ARCHVIEW_3D_CAPTURE") {
                    if !CAPTURED.swap(true,Ordering::Relaxed) {
                        let viewport=info.viewport_in_pixels();
                        let mut pixels=vec![0;viewport.width_px as usize*viewport.height_px as usize*4];
                        gl.pixel_store_i32(glow::PACK_ALIGNMENT,1);
                        gl.read_pixels(viewport.left_px,viewport.from_bottom_px,viewport.width_px,viewport.height_px,glow::RGBA,glow::UNSIGNED_BYTE,glow::PixelPackData::Slice(&mut pixels));
                        if let Some(image)=image::RgbaImage::from_raw(viewport.width_px as u32,viewport.height_px as u32,pixels) {
                            let _=image::imageops::flip_vertical(&image).save(path);
                        }
                    }
                }
                gl.scissor(previous_scissor[0],previous_scissor[1],previous_scissor[2],previous_scissor[3]);
            }
        }
    }))});
    ui.painter().text(rect.left_bottom()+egui::vec2(8.0,-8.0),egui::Align2::LEFT_BOTTOM,"Drag: orbit · Shift+drag: pan · Wheel: zoom · Double-click: fullscreen",egui::FontId::proportional(12.0),egui::Color32::LIGHT_GRAY);
    response.double_clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_formats_import_geometry_and_render_nonempty_thumbnails() {
        let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/models");
        for ext in ["obj","gltf","glb","fbx","usd","usda","usdc","usdz"] {
            let model=load(&root.join(format!("sample.{ext}"))).unwrap_or_else(|error|panic!("{ext}: {error:#}"));
            assert!(model.triangles>100);
            assert_eq!(model.vertices.len(),model.triangles*27);
            let image=thumbnail(&model);
            let background=egui::Color32::from_rgb(29,32,38);
            assert!(image.pixels.iter().filter(|pixel|**pixel!=background).count()>1000,"Empty {ext} preview");
            let cached=load(&root.join(format!("sample.{ext}"))).unwrap();
            assert_eq!(cached.vertices,model.vertices);
        }
    }
    #[test]
    fn malformed_mesh_data_is_rejected() {
        assert!(Model::from_bytes(b"bad").is_err());
        let mut bytes=b"AV3D0001".to_vec();bytes.extend_from_slice(&1_u32.to_le_bytes());
        assert!(Model::from_bytes(&bytes).is_err());
        bytes.extend_from_slice(&vec![0;108]);
        bytes[12..16].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(Model::from_bytes(&bytes).is_err());
    }
    #[test]
    fn mouse_controls_orbit_pan_and_zoom() {
        let ctx=egui::Context::default();let model=Arc::new(Model{vertices:vec![0.0;27],triangles:1});
        let renderer=Arc::new(Mutex::new(None));let mut view=View::default();
        let mut frame=|events,modifiers| {
            let input=egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(500.0,500.0))),events,modifiers,..Default::default()};
            let _=ctx.run(input,|ctx|{egui::CentralPanel::default().show(ctx,|ui|{show(ui,model.clone(),&mut view,renderer.clone(),&crate::settings::Settings::default());});});
        };
        let down=egui::pos2(200.0,200.0);let moved=egui::pos2(250.0,230.0);
        frame(vec![],Default::default());
        frame(vec![egui::Event::PointerMoved(down),egui::Event::PointerButton{pos:down,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}],Default::default());
        frame(vec![egui::Event::PointerMoved(moved)],Default::default());
        let shift=egui::Modifiers{shift:true,..Default::default()};
        frame(vec![egui::Event::PointerMoved(egui::pos2(270.0,240.0))],shift);
        frame(vec![egui::Event::MouseWheel{unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.0,100.0),modifiers:Default::default()}],Default::default());
        assert!(view.yaw<View::default().yaw);assert!(view.pitch<View::default().pitch);assert_ne!(view.pan,[0.0;2]);assert!(view.zoom>View::default().zoom);
    }
}
