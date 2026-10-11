use super::*;
use std::time::{Duration, Instant};

struct Bench {
    files: Vec<usize>,
    visits: Vec<(usize, &'static str, u64)>,
    cursor: usize,
    started: Option<Instant>,
    ready: Option<(f64, u64)>,
    next: Instant,
    samples: Vec<String>,
    initialized: bool,
}
thread_local! { static STATE: std::cell::RefCell<Option<Bench>> = const { std::cell::RefCell::new(None) }; }
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}
pub fn tick(app: &mut App, ctx: &egui::Context) {
    if std::env::var_os("VIEW_CALIPERS_BENCH_FOLDER").is_none() { return; }
    ctx.request_repaint();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let bench = state.get_or_insert_with(|| Bench {
            files: vec![], visits: vec![], cursor: 0, started: None, ready: None,
            next: Instant::now(), samples: vec![], initialized: false,
        });
        if !bench.initialized {
            if app.pending_open.is_some() || app.images.is_empty() { return; }
            if app.thumbs.iter().enumerate().any(|(i, t)| t.is_none() && !app.thumb_errors.contains_key(&i)) { return; }
            let mut files: Vec<usize> = (0..app.images.len()).filter(|&i| image_pipeline::is_photo(&app.images[i].src)).collect();
            files.sort_by(|&a,&b| app.images[a].label.cmp(&app.images[b].label));
            files.truncate(12);
            if files.len() != 12 { return; }
            bench.files = files;
            for i in 0..12 { bench.visits.push((i, "first-forward", 160)); }
            for i in (0..11).rev() { bench.visits.push((i, "return", 160)); }
            for i in 0..12 { bench.visits.push((i, "rapid-forward", 0)); }
            for i in (0..11).rev() { bench.visits.push((i, "rapid-return", 0)); }
            // Match the WebView's fullscreen content layout at 1600x1000 without
            // changing the physical desktop resolution or maximizing the window.
            app.fullscreen = true;
            bench.initialized = true;
        }
        if let Some(started) = bench.started {
            let (index, phase, pause) = bench.visits[bench.cursor];
            if app.current != Some(bench.files[index]) || app.texture.is_none() { return; }
            let pass = ctx.cumulative_pass_nr();
            let (ready_ms, ready_pass) = *bench.ready.get_or_insert_with(|| (started.elapsed().as_secs_f64()*1000.0, pass));
            // Count the ready pass and the following pass, like two RAF callbacks. This is a
            // rendering opportunity estimate, not a GPU completion fence.
            if pass < ready_pass + 1 { return; }
            bench.samples.push(format!("{{\"phase\":{},\"name\":{},\"decodeReadyMs\":{},\"presentedMs\":{}}}", quote(phase), quote(&app.images[bench.files[index]].label), ready_ms, started.elapsed().as_secs_f64()*1000.0));
            bench.cursor += 1;
            bench.started = None;
            bench.ready = None;
            bench.next = Instant::now() + Duration::from_millis(pause);
        }
        if bench.cursor == bench.visits.len() {
            let files = bench.files.iter().map(|&i| {
                let bytes = match &app.images[i].src { ImageSrc::File(p) => std::fs::metadata(p).map_or(0, |m|m.len()), _=>0 };
                format!("{{\"name\":{},\"bytes\":{bytes}}}", quote(&app.images[i].label))
            }).collect::<Vec<_>>().join(",");
            let size=ctx.input(|i|i.screen_rect().size());
            let report=format!("{{\"mode\":\"rust-egui\",\"files\":[{files}],\"samples\":[{}],\"viewport\":[{},{}],\"photoRamMb\":{},\"photoGpuMb\":{},\"preload\":{},\"note\":\"Ready texture plus two egui passes; not GPU fence. Filesystem cache not cleared.\"}}",bench.samples.join(","),size.x,size.y,app.settings.photo_ram_mb,app.settings.photo_gpu_mb,app.settings.photo_preload);
            if let Some(path)=std::env::var_os("VIEW_CALIPERS_BENCH_OUTPUT") { std::fs::write(path, report).expect("benchmark report"); }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if bench.started.is_none() && Instant::now() >= bench.next {
            let index=bench.files[bench.visits[bench.cursor].0];
            bench.started=Some(Instant::now());
            app.goto(index, ctx);
            if app.texture.is_some() {
                bench.ready = Some((bench.started.unwrap().elapsed().as_secs_f64()*1000.0, ctx.cumulative_pass_nr()));
            }
        }
    });
}
