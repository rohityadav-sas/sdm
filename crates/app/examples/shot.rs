//! Headless screenshot tool: runs the real app (engine + local server) with the
//! software renderer and writes PNGs.  Usage: shot <outdir>
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};
use std::rc::Rc;
use std::time::{Duration, Instant};
use sdm_app::devserver::serve;
use sdm_app::store::{save_history, Saved};
use sdm_app::{App, Config, MainWindow, Theme};

thread_local! { static START: Instant = Instant::now(); }

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> Duration {
        START.with(|s| s.elapsed())
    }
}

fn advance(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn save(win: &MinimalSoftwareWindow, w: u32, h: u32, path: &str) {
    let mut buf = vec![Rgb8Pixel { r: 0x77, g: 0x77, b: 0x77 }; (w * h) as usize];
    win.request_redraw();
    win.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let mut bytes = Vec::with_capacity(buf.len() * 3);
    for p in &buf {
        bytes.extend_from_slice(&[p.r, p.g, p.b]);
    }
    let f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(f, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&bytes).unwrap();
}

fn saved(id: i32, name: &str, total_mb: u64, done_mb: u64, complete: bool, ago: u64) -> Saved {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    Saved {
        id, url: format!("https://downloads.example.com/{name}"), name: name.into(), dir: "/tmp".into(),
        path: Some(format!("/tmp/{name}")), total: total_mb << 20, done: done_mb << 20, complete,
        connections: 8, finished: complete.then(|| now - ago), headers: Vec::new(),
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/shots".into());
    std::fs::create_dir_all(&out).unwrap();
    let (w, h) = (1040u32, 720u32);
    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();
    win.set_size(PhysicalSize::new(w, h));

    let data = std::env::temp_dir().join(format!("sdm-shot-data-{}", std::process::id()));
    let dl = std::env::temp_dir().join(format!("sdm-shot-dl-{}", std::process::id()));
    std::fs::create_dir_all(&dl).unwrap();
    save_history(&data, &[
        saved(3, "Ambient Focus Mix 2026.flac", 312, 168, false, 0),
        saved(2, "design-assets-pack.zip", 86, 86, true, 240),
        saved(1, "Annual Report 2025.pdf", 14, 14, true, 3700),
    ]);

    let ui = MainWindow::new().unwrap();
    let app = App::install(&ui, Config { data_dir: data.clone(), ipc: false });
    ui.set_download_folder(dl.to_string_lossy().into_owned().into());
    ui.show().unwrap();

    let big = serve(3_000_000_000, 8_000_000);
    let mid = serve(1_480_000_000, 1_800_000);
    ui.set_max_active(2);
    ui.set_default_conns(8);
    app.add_url(&format!("{big}/Blender-Demo-Render-4K.mp4"));
    ui.set_default_conns(16);
    app.add_url("http://127.0.0.1:9/wallpaper-aurora-4k.png");
    app.add_url(&format!("{big}/ubuntu-24.04.3-desktop-amd64.iso"));
    ui.set_default_conns(4);
    app.add_url(&format!("{mid}/conference-keynote-2026.mkv"));
    advance(2500);
    ui.global::<Theme>().set_mode(1);
    advance(400);
    save(&win, w, h, &format!("{out}/1-light.png"));

    ui.global::<Theme>().set_mode(2);
    advance(400);
    save(&win, w, h, &format!("{out}/2-dark.png"));

    ui.set_dialog_open(true);
    ui.set_dialog_url("https://example.com/files/dataset-v3.tar.gz".into());
    advance(400);
    save(&win, w, h, &format!("{out}/3-dialog-dark.png"));
    ui.set_dialog_open(false);
    advance(300);

    ui.global::<Theme>().set_mode(1);
    ui.set_page(4);
    win.set_size(PhysicalSize::new(w, 960));
    advance(500);
    save(&win, w, 960, &format!("{out}/4-preferences-light.png"));
    ui.set_page(5);
    advance(300);
    save(&win, w, 960, &format!("{out}/5-extension-light.png"));
    win.set_size(PhysicalSize::new(w, h));
    ui.set_page(2);
    advance(300);
    save(&win, w, h, &format!("{out}/6-completed-light.png"));
    ui.set_page(1);
    ui.global::<Theme>().set_mode(2);
    advance(300);
    save(&win, w, h, &format!("{out}/7-active-dark.png"));
    println!("saved to {out}");
    drop(app);
    let _ = std::fs::remove_dir_all(&dl);
    let _ = std::fs::remove_dir_all(&data);
}
