//! Headless Slint platform shared by the end-to-end tests: software renderer,
//! real-time clock, clipboard, manually pumped event loop and a "did the app quit?" flag.
#![allow(dead_code)]
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Clipboard, EventLoopProxy, Platform, WindowAdapter};
use slint::{Model, PhysicalSize};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use sdm_app::MainWindow;

type Job = Box<dyn FnOnce() + Send>;
static QUEUE: OnceLock<Arc<Mutex<VecDeque<Job>>>> = OnceLock::new();
pub static QUIT: AtomicBool = AtomicBool::new(false);
thread_local! {
    pub static CLIP: RefCell<Option<String>> = const { RefCell::new(None) };
    static START: Instant = Instant::now();
}

struct Proxy;
impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        QUIT.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn invoke_from_event_loop(&self, f: Job) -> Result<(), slint::EventLoopError> {
        QUEUE.get().unwrap().lock().unwrap().push_back(f);
        Ok(())
    }
}

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> Duration {
        START.with(|s| s.elapsed())
    }
    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy))
    }
    fn clipboard_text(&self, _c: Clipboard) -> Option<String> {
        CLIP.with(|c| c.borrow().clone())
    }
    fn set_clipboard_text(&self, text: &str, _c: Clipboard) {
        CLIP.with(|c| *c.borrow_mut() = Some(text.to_string()));
    }
}

pub fn setup() -> Rc<MinimalSoftwareWindow> {
    QUEUE.set(Arc::new(Mutex::new(VecDeque::new()))).ok();
    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();
    // What slint::run_event_loop_until_quit() does in the real app: keep the event
    // loop alive after the last window is closed.
    #[allow(deprecated)]
    P(win.clone()).set_event_loop_quit_on_last_window_closed(false);
    win.set_size(PhysicalSize::new(1040, 720));
    win
}

pub fn pump() {
    while let Some(job) = QUEUE.get().unwrap().lock().unwrap().pop_front() {
        job();
    }
    slint::platform::update_timers_and_animations();
}

pub fn pump_until(timeout_s: u64, what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(timeout_s);
    while Instant::now() < end {
        pump();
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    panic!("timed out waiting for: {what}");
}

pub fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump();
        std::thread::sleep(Duration::from_millis(15));
    }
}

pub fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sdm-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn status(ui: &MainWindow, i: usize) -> String {
    ui.get_downloads().row_data(i).map(|d| d.status.to_string()).unwrap_or_default()
}

pub fn progress(ui: &MainWindow, i: usize) -> f32 {
    ui.get_downloads().row_data(i).map(|d| d.progress).unwrap_or(0.0)
}
