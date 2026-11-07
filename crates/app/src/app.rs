//! Glue between the Slint UI and the download engine.
//!
//! * Downloads run on a small tokio runtime; each task publishes progress into
//!   atomics. A UI-thread timer (only while something is running) turns those
//!   into model rows, so the UI never blocks and idle CPU stays at zero.
//! * Settings and the download list are persisted as small JSON files.

use crate::format::*;
use crate::store::{self, Saved, Settings};
use crate::{sys, DownloadItem, MainWindow, Theme};
use engine::{CancellationToken, Info, Options, Outcome, RateLimiter};
use slint::winit_030::{winit, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::{Rc, Weak as RcWeak};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Config {
    /// Where settings.json, downloads.json and ipc.json live.
    pub data_dir: PathBuf,
    /// Listen for the browser extension / command line (see the `ipc` crate).
    pub ipc: bool,
}

impl Config {
    pub fn from_system() -> Self {
        Self { data_dir: ipc::data_dir(), ipc: true }
    }
}

/// Speed-limit choices, indexed by the slider (bytes per second, 0 = unlimited).
pub const SPEED_LIMITS: [u64; 8] = [0, 512 << 10, 1 << 20, 2 << 20, 5 << 20, 10 << 20, 20 << 20, 50 << 20];

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const PAUSED: u8 = 2;
const FAILED: u8 = 3;

/// Shared between a download task (any thread) and the UI timer.
struct Task {
    downloaded: AtomicU64,
    total: AtomicU64,
    conns: AtomicUsize,
    state: AtomicU8,
    name: Mutex<Option<String>>,
    path: Mutex<Option<PathBuf>>,
    err: Mutex<String>,
    cancel: CancellationToken,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum St {
    Queued,
    Running,
    Paused,
    Done,
    Error,
}

struct Item {
    id: i32,
    url: String,
    name: String,
    dir: PathBuf,
    path: Option<PathBuf>,
    total: u64,
    done: u64,
    conns: i32,
    st: St,
    pausing: bool,
    /// Resume was requested while a pause was still finishing.
    resume_after: bool,
    speed: f64,
    last_done: u64,
    active: usize,
    err: String,
    finished: Option<u64>,
    task: Option<Arc<Task>>,
    headers: Vec<(String, String)>,
    last_pushed_done: u64,
    last_pushed_speed_fmt: String,
    last_pushed_st: St,
    last_pushed_active: usize,
}

struct Inner {
    ui: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    client: engine::Client,
    limiter: Arc<RateLimiter>,
    items: Vec<Item>,
    model: Rc<VecModel<DownloadItem>>,
    next_id: i32,
    cfg: Config,
    tick: Timer,
    save: Timer,
    update_timer: Timer,
    last_tick: Instant,
    last_save: Instant,
    /// The window was closed while downloads keep running.
    background: bool,
    /// Recent total speed (bytes/s), for the sparkline.
    speeds: VecDeque<f32>,
    last_sample: Instant,
    me: RcWeak<RefCell<Inner>>,
    pending_update: Option<crate::update::UpdateInfo>,
    last_win_size: Rc<Cell<(i32, i32)>>,
}

pub struct App {
    inner: Rc<RefCell<Inner>>,
    _slow: Timer,
    _settings_timer: Rc<Timer>,
    _ipc: Option<ipc::Server>,
    _rt: tokio::runtime::Runtime,
}

thread_local! {
    /// How IPC requests (arriving on a background thread) find the UI-thread state.
    static INNER: RefCell<Option<RcWeak<RefCell<Inner>>>> = const { RefCell::new(None) };
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum AddResult {
    Added,
    Existing,
    Invalid,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn to_row(it: &Item) -> DownloadItem {
    let progress = if it.total == 0 { 0.0 } else { (it.done as f32 / it.total as f32).min(1.0) };
    let st = if it.pausing && !it.resume_after { St::Paused } else { it.st };
    let (status, speed, detail) = match st {
        St::Running => {
            let detail = if it.done == 0 && it.speed < 1.0 {
                "Connecting…".to_string()
            } else {
                let n = it.active.max(1);
                let cn = if n == 1 { "1 connection".to_string() } else { format!("{n} connections") };
                if it.total > 0 && it.speed > 1.0 {
                    format!("{cn} · {}", fmt_eta(((it.total - it.done.min(it.total)) as f64 / it.speed) as u64))
                } else {
                    cn
                }
            };
            ("downloading", fmt_speed(it.speed), detail)
        }
        St::Queued => ("queued", String::new(), "Waiting for a free slot".to_string()),
        St::Paused => ("paused", String::new(), format!("Paused at {:.0}%", progress * 100.0)),
        St::Done => {
            let ago = it.finished.map(|f| fmt_ago(now_secs().saturating_sub(f))).unwrap_or_default();
            ("done", String::new(), format!("Finished {ago}"))
        }
        St::Error => ("error", String::new(), it.err.clone()),
    };
    DownloadItem {
        id: it.id,
        name: it.name.clone().into(),
        kind: kind_of(&it.name).into(),
        status: status.into(),
        progress,
        downloaded: if it.total == 0 && it.done == 0 { "Size not known yet".into() } else { fmt_size(it.done) }.into(),
        total: if it.total > 0 { fmt_size(it.total) } else { String::new() }.into(),
        speed: speed.into(),
        detail: detail.into(),
        conns: it.conns,
    }
}

impl Inner {
    fn toast(&self, msg: &str) {
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_show_toast(msg.into());
        }
    }

    fn idx(&self, id: i32) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    fn push_row(&mut self, i: usize) {
        let it = &mut self.items[i];
        let row = to_row(it);
        it.last_pushed_done = it.done;
        it.last_pushed_speed_fmt = row.speed.to_string();
        it.last_pushed_st = it.st;
        it.last_pushed_active = it.active;
        self.model.set_row_data(i, row);
    }

    fn refresh_stats(&self) -> f64 {
        let mut total_speed = 0.0f64;
        let mut active = 0i32;
        let mut done = 0i32;
        let mut busy = 0i32;
        let mut stopped = 0i32;
        let all = self.items.len() as i32;

        for it in &self.items {
            match it.st {
                St::Running => {
                    if !it.pausing {
                        total_speed += it.speed;
                        active += 1;
                    }
                    busy += 1;
                }
                St::Queued => {
                    busy += 1;
                }
                St::Done => {
                    done += 1;
                }
                St::Paused | St::Error => {
                    stopped += 1;
                }
            }
        }

        if let Some(ui) = self.ui.upgrade() {
            let speed_str = fmt_speed(total_speed);
            if ui.get_total_speed() != speed_str {
                ui.set_total_speed(speed_str.into());
            }
            if ui.get_active_count() != active {
                ui.set_active_count(active);
            }
            if ui.get_done_count() != done {
                ui.set_done_count(done);
            }
            if ui.get_count_all() != all {
                ui.set_count_all(all);
            }
            if ui.get_count_busy() != busy {
                ui.set_count_busy(busy);
            }
            if ui.get_count_stopped() != stopped {
                ui.set_count_stopped(stopped);
            }
        }
        total_speed
    }

    fn ensure_tick(&self) {
        if self.tick.running() {
            return;
        }
        let me = self.me.clone();
        self.tick.start(TimerMode::Repeated, Duration::from_millis(120), move || {
            if let Some(rc) = me.upgrade() {
                if let Ok(mut g) = rc.try_borrow_mut() {
                    g.tick();
                }
            }
        });
    }

    fn save_soon(&self) {
        let me = self.me.clone();
        self.save.start(TimerMode::SingleShot, Duration::from_millis(500), move || {
            if let Some(rc) = me.upgrade() {
                if let Ok(g) = rc.try_borrow() {
                    g.save_now();
                }
            }
        });
    }

    fn collect_saved(&self) -> Vec<Saved> {
        self.items
            .iter()
            .map(|i| Saved {
                id: i.id,
                url: i.url.clone(),
                name: i.name.clone(),
                dir: i.dir.to_string_lossy().into_owned(),
                path: i.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                total: i.total,
                done: i.done,
                complete: i.st == St::Done,
                connections: i.conns,
                finished: i.finished,
                headers: i.headers.clone(),
            })
            .collect()
    }

    fn save_now(&self) {
        let list = self.collect_saved();
        let dir = self.cfg.data_dir.clone();
        self.rt.spawn_blocking(move || {
            store::save_history(&dir, &list);
        });
    }

    fn save_sync(&self) {
        let list = self.collect_saved();
        store::save_history(&self.cfg.data_dir, &list);
    }

    fn start_task(&mut self, idx: usize) {
        let task = Arc::new(Task {
            downloaded: AtomicU64::new(self.items[idx].done),
            total: AtomicU64::new(self.items[idx].total),
            conns: AtomicUsize::new(0),
            state: AtomicU8::new(RUNNING),
            name: Mutex::new(None),
            path: Mutex::new(None),
            err: Mutex::new(String::new()),
            cancel: CancellationToken::new(),
        });
        let it = &mut self.items[idx];
        let (client, url, dir, conns) = (self.client.clone(), it.url.clone(), it.dir.clone(), it.conns.clamp(1, 32) as usize);
        let t_info = task.clone();
        let opts = Options {
            connections: conns,
            limiter: Some(self.limiter.clone()),
            headers: it.headers.clone(),
            on_info: Some(Arc::new(move |i: Info| {
                *t_info.name.lock().unwrap() = Some(i.filename);
                *t_info.path.lock().unwrap() = Some(i.path);
                if let Some(t) = i.total {
                    t_info.total.store(t, Relaxed);
                }
            })),
        };
        let t = task.clone();
        self.rt.spawn(async move {
            let t2 = t.clone();
            let res = engine::download(&client, &url, &dir, &opts, &t.cancel, move |p| {
                t2.downloaded.store(p.downloaded, Relaxed);
                if let Some(total) = p.total {
                    t2.total.store(total, Relaxed);
                }
                t2.conns.store(p.connections, Relaxed);
            })
            .await;
            match res {
                Ok(Outcome::Done(path)) => {
                    *t.path.lock().unwrap() = Some(path);
                    t.state.store(DONE, Relaxed);
                }
                Ok(Outcome::Paused) => t.state.store(PAUSED, Relaxed),
                Err(e) => {
                    *t.err.lock().unwrap() = friendly(&e);
                    t.state.store(FAILED, Relaxed);
                }
            }
        });
        it.task = Some(task);
        it.st = St::Running;
        it.pausing = false;
        it.speed = 0.0;
        it.last_done = it.done;
        it.err.clear();
        self.last_tick = Instant::now();
        self.ensure_tick();
        self.push_row(idx);
        self.refresh_stats();
    }

    fn add(&mut self, url: &str, dir: Option<PathBuf>, conns: Option<i32>) -> bool {
        self.add_inner(url, dir, conns, Vec::new(), None) == AddResult::Added
    }

    fn add_inner(&mut self, url: &str, dir: Option<PathBuf>, conns: Option<i32>, headers: Vec<(String, String)>, name_hint: Option<String>) -> AddResult {
        let url = url.trim().to_string();
        let Some(ui) = self.ui.upgrade() else { return AddResult::Invalid };
        match engine::Url::parse(&url) {
            Ok(u) if u.scheme() == "http" || u.scheme() == "https" => {}
            _ => {
                self.toast(if url.is_empty() { "Nothing to download yet" } else { "That doesn't look like a web link" });
                return AddResult::Invalid;
            }
        }
        if let Some(i) = self.items.iter().position(|i| i.url == url && i.st != St::Done) {
            if !headers.is_empty() {
                self.items[i].headers = headers; // fresher cookies
            }
            if self.items[i].st != St::Running && self.items[i].task.is_none() {
                self.start_task(i);
                self.toast("Resuming download");
            } else {
                self.toast("That download is already in your list");
            }
            return AddResult::Existing;
        }
        let dir = dir.unwrap_or_else(|| PathBuf::from(ui.get_download_folder().as_str()));
        let conns = conns.unwrap_or_else(|| ui.get_default_conns());
        let id = self.next_id;
        self.next_id += 1;
        let name = name_hint.filter(|n| !n.is_empty()).unwrap_or_else(|| name_from_url(&url));
        let mut item = Item {
            id, url, name, dir, path: None, total: 0, done: 0, conns, st: St::Running, pausing: false, resume_after: false,
            speed: 0.0, last_done: 0, active: 0, err: String::new(), finished: None, task: None, headers,
            last_pushed_done: 0,
            last_pushed_speed_fmt: String::new(),
            last_pushed_st: St::Running,
            last_pushed_active: 0,
        };
        let queued = self.running_count() >= self.max_active();
        if queued {
            item.st = St::Queued;
        }
        self.model.insert(0, to_row(&item));
        self.items.insert(0, item);
        if queued {
            self.toast("Added to the queue");
        } else {
            self.start_task(0);
        }
        self.refresh_stats();
        self.save_soon();
        AddResult::Added
    }

    /// Requests from the browser extension and the command line (UI thread).
    fn handle_ipc(&mut self, req: ipc::Request) -> ipc::Reply {
        let from_extension = req.source.as_deref() == Some("extension");
        if from_extension {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_ext_state(2);
            }
        }
        match req.kind.as_str() {
            "ping" => ipc::Reply { ok: true, version: Some(env!("CARGO_PKG_VERSION").into()), ..Default::default() },
            "show" => {
                self.raise();
                ipc::Reply::ok()
            }
            "add" => {
                let headers: Vec<(String, String)> = req.headers.into_iter().collect();
                match self.add_inner(&req.url, None, None, headers, req.filename.clone()) {
                    AddResult::Invalid => ipc::Reply::err("Not a web link"),
                    AddResult::Existing => ipc::Reply::ok(),
                    AddResult::Added => {
                        let name = req.filename.filter(|n| !n.is_empty()).unwrap_or_else(|| name_from_url(&req.url));
                        if from_extension {
                            self.toast(&format!("Captured {name} from your browser"));
                            if self.ui.upgrade().map(|u| u.get_notifications()).unwrap_or(false) {
                                sys::notify("SDM captured a download", &name);
                            }
                        }
                        ipc::Reply::ok()
                    }
                }
            }
            other => ipc::Reply::err(format!("unknown request '{other}'")),
        }
    }

    fn raise(&mut self) {
        self.background = false;
        for i in 0..self.items.len() {
            self.push_row(i);
        }
        self.refresh_stats();
        if let Some(ui) = self.ui.upgrade() {
            ui.window().set_minimized(false);
            let _ = ui.show();
        }
    }

    /// Register the native host with the installed browsers and unpack the extension.
    fn setup_extension(&mut self) {
        let host_name = if cfg!(windows) { "sdm-host.exe" } else { "sdm-host" };
        let host = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(host_name))).filter(|p| p.exists());
        let Some(host) = host else {
            self.toast("sdm-host is missing next to the app (run cargo build --release)");
            return;
        };
        let names = match ipc::install::install_native_host(&host) {
            Ok(n) => n,
            Err(e) => {
                self.toast(&format!("Setup failed: {e}"));
                return;
            }
        };
        if names.is_empty() {
            self.toast("No supported browser found. Open one once, then try again");
            return;
        }
        let dir = self.cfg.data_dir.join("extension");
        if let Err(e) = ipc::install::extract_extension(&dir) {
            self.toast(&format!("Couldn't write the extension files: {e}"));
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            if ui.get_ext_state() < 1 {
                ui.set_ext_state(1);
            }
        }
        sys::open(&dir);
        self.toast(&format!("Linked with {}. Now load the extension", names.join(", ")));
    }

    fn pause_all(&mut self) {
        for i in 0..self.items.len() {
            match self.items[i].st {
                St::Running => {
                    if let Some(t) = &self.items[i].task {
                        t.cancel.cancel();
                    }
                    self.items[i].pausing = true;
                    self.items[i].resume_after = false;
                    self.items[i].speed = 0.0;
                    self.push_row(i);
                }
                St::Queued => {
                    self.items[i].st = St::Paused;
                    self.push_row(i);
                }
                _ => {}
            }
        }
        self.refresh_stats();
        self.save_soon();
    }

    /// Everything paused goes back in the queue (oldest first) and starts as slots allow.
    fn resume_all(&mut self) {
        let mut n = 0;
        for i in (0..self.items.len()).rev() {
            let it = &mut self.items[i];
            if it.st == St::Running && it.pausing && !it.resume_after {
                it.resume_after = true; // still stopping; comes back by itself
                n += 1;
                self.push_row(i);
            } else if it.st == St::Paused && it.task.is_none() && !it.pausing {
                it.st = St::Queued;
                n += 1;
                self.push_row(i);
            }
        }
        self.pump_queue();
        self.refresh_stats();
        if n > 0 {
            self.toast(&format!("Resuming {n} download{}", if n == 1 { "" } else { "s" }));
        }
        self.save_soon();
    }

    /// Remove finished downloads from the list (the files stay on disk).
    fn clear_done(&mut self) {
        let mut n = 0;
        for i in (0..self.items.len()).rev() {
            if self.items[i].st == St::Done {
                self.items.remove(i);
                self.model.remove(i);
                n += 1;
            }
        }
        self.refresh_stats();
        self.save_soon();
        self.toast(&if n == 0 { "Nothing to clear".to_string() } else { format!("Cleared {n} completed") });
    }

    fn open_download_folder(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let dir = PathBuf::from(ui.get_download_folder().as_str());
        let _ = std::fs::create_dir_all(&dir);
        sys::open(&dir);
    }

    fn busy(&self) -> bool {
        self.items.iter().any(|i| i.task.is_some() || i.st == St::Queued)
    }

    /// The window was closed: keep downloading in the background, or quit.
    fn on_window_close(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        store::save_settings(&self.cfg.data_dir, &settings_from_ui(&ui, &self.last_win_size));
        if ui.get_background_mode() && self.busy() {
            self.background = true;
            if ui.get_notifications() {
                sys::notify("SDM keeps downloading", "Open SDM again to see progress. It quits when everything is done.");
            }
        } else {
            self.quit();
        }
    }

    fn quit(&mut self) {
        if let Some(ui) = self.ui.upgrade() {
            store::save_settings(&self.cfg.data_dir, &settings_from_ui(&ui, &self.last_win_size));
        }
        self.shutdown();
        let _ = slint::quit_event_loop();
    }

    fn sample_speed(&mut self, bytes_per_sec: f64) {
        if self.last_sample.elapsed() < Duration::from_millis(400) {
            return;
        }
        self.last_sample = Instant::now();
        self.speeds.push_back(bytes_per_sec as f32);
        while self.speeds.len() > 60 {
            self.speeds.pop_front();
        }
        self.refresh_spark();
    }

    fn refresh_spark(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if self.speeds.len() < 2 {
            ui.set_speed_path("".into());
            ui.set_speed_fill("".into());
            return;
        }
        let (w, h) = (184.0f32, 36.0f32);
        let max = self.speeds.iter().cloned().fold(1_048_576.0f32, f32::max);
        let step = w / (self.speeds.len() - 1) as f32;
        let mut line = String::new();
        for (i, v) in self.speeds.iter().enumerate() {
            let (x, y) = (i as f32 * step, h - 3.0 - (v / max) * (h - 8.0));
            line.push_str(&format!("{}{:.1} {:.1} ", if i == 0 { "M" } else { "L" }, x, y));
        }
        let fill = format!("{line}L{w:.1} {h:.1} L0 {h:.1} Z");
        ui.set_speed_path(line.trim().into());
        ui.set_speed_fill(fill.into());
    }

    fn running_count(&self) -> usize {
        self.items.iter().filter(|i| i.task.is_some()).count()
    }

    fn max_active(&self) -> usize {
        self.ui.upgrade().map(|u| u.get_max_active().clamp(1, 8) as usize).unwrap_or(4)
    }

    /// Start waiting downloads (oldest first) while there are free slots.
    fn pump_queue(&mut self) {
        let max = self.max_active();
        while self.running_count() < max {
            let Some(i) = self.items.iter().rposition(|i| i.st == St::Queued && i.task.is_none()) else { break };
            self.start_task(i);
        }
    }

    fn apply_limits(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let idx = ui.get_speed_limit().clamp(0, SPEED_LIMITS.len() as i32 - 1) as usize;
        self.limiter.set_rate(SPEED_LIMITS[idx]);
        self.pump_queue();
    }

    fn toggle(&mut self, id: i32) {
        let Some(i) = self.idx(id) else { return };
        match self.items[i].st {
            St::Running if self.items[i].pausing => {
                // Still stopping: remember that the user wants it back.
                self.items[i].resume_after = !self.items[i].resume_after;
                self.push_row(i);
            }
            St::Running => {
                if let Some(t) = &self.items[i].task {
                    t.cancel.cancel();
                }
                self.items[i].pausing = true;
                self.items[i].resume_after = false;
                self.items[i].speed = 0.0;
                self.push_row(i);
                self.refresh_stats();
            }
            // Paused, failed, or waiting in the queue: the user asked for it now.
            St::Paused | St::Error | St::Queued => {
                if self.items[i].task.is_none() {
                    self.start_task(i);
                }
            }
            St::Done => {}
        }
    }

    fn remove(&mut self, id: i32) {
        let Some(i) = self.idx(id) else { return };
        let it = self.items.remove(i);
        self.model.remove(i);
        if it.st != St::Done {
            if let Some(t) = &it.task {
                t.cancel.cancel();
            }
            // Drop leftovers of an unfinished download once its task has stopped.
            if let Some(p) = it.path.clone() {
                self.rt.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    let mut part = p.clone().into_os_string();
                    part.push(".part");
                    let mut meta = p.into_os_string();
                    meta.push(".part.json");
                    let _ = std::fs::remove_file(part);
                    let _ = std::fs::remove_file(meta);
                });
            }
        }
        self.pump_queue();
        self.refresh_stats();
        self.save_soon();
    }

    fn open_folder(&self, id: i32) {
        let Some(i) = self.idx(id) else { return };
        let it = &self.items[i];
        let dir = it.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| it.dir.clone());
        if dir.exists() {
            sys::open(&dir);
        } else {
            self.toast("That folder doesn't exist yet");
        }
    }

    fn open_file(&self, id: i32) {
        let Some(i) = self.idx(id) else { return };
        match &self.items[i].path {
            Some(p) if p.exists() => sys::open(p),
            _ => self.toast("The file is no longer there"),
        }
    }

    fn copy_link(&self, id: i32) {
        let Some(i) = self.idx(id) else { return };
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_write_clipboard(self.items[i].url.clone().into());
            self.toast("Link copied");
        }
    }

    fn paste(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let text = ui.invoke_read_clipboard();
        let links = links_in(&text);
        if links.is_empty() {
            self.toast("No download link found in the clipboard");
            return;
        }
        let mut added = 0;
        let mut first = String::new();
        for l in &links {
            if self.add(l, None, None) {
                added += 1;
                if first.is_empty() {
                    first = name_from_url(l);
                }
            }
        }
        match added {
            0 => {}
            1 => self.toast(&format!("Capturing {first}")),
            n => self.toast(&format!("Capturing {n} downloads")),
        }
    }

    /// Runs on the UI thread while at least one download is active.
    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().max(0.001);
        self.last_tick = now;
        let mut changed = false;
        let mut finished: Vec<String> = Vec::new();

        for i in 0..self.items.len() {
            let Some(t) = self.items[i].task.clone() else { continue };
            let d = t.downloaded.load(Relaxed);
            let total = t.total.load(Relaxed);
            let mut state_changed = false;
            let mut name_to_finish = None;

            {
                let it = &mut self.items[i];
                if it.path.is_none() {
                    if let Some(n) = t.name.lock().unwrap().clone() {
                        it.name = n;
                    }
                    if let Some(p) = t.path.lock().unwrap().clone() {
                        it.path = Some(p);
                    }
                }
                if total > 0 {
                    it.total = total;
                }
                let inst = d.saturating_sub(it.last_done) as f64 / dt;
                it.speed = if it.speed < 1.0 { inst } else { 0.6 * it.speed + 0.4 * inst };
                it.last_done = d;
                it.done = d;
                it.active = t.conns.load(Relaxed);
                match t.state.load(Relaxed) {
                    RUNNING => {}
                    DONE => {
                        it.st = St::Done;
                        it.done = it.total.max(d);
                        it.speed = 0.0;
                        it.finished = Some(now_secs());
                        it.task = None;
                        it.pausing = false;
                        name_to_finish = Some(it.name.clone());
                        changed = true;
                        state_changed = true;
                    }
                    PAUSED => {
                        it.st = if it.resume_after { St::Queued } else { St::Paused };
                        it.resume_after = false;
                        it.speed = 0.0;
                        it.task = None;
                        it.pausing = false;
                        changed = true;
                        state_changed = true;
                    }
                    _ => {
                        it.st = St::Error;
                        it.err = t.err.lock().unwrap().clone();
                        it.speed = 0.0;
                        it.task = None;
                        it.pausing = false;
                        changed = true;
                        state_changed = true;
                    }
                }
            }

            if let Some(n) = name_to_finish {
                finished.push(n);
            }

            let should_push = {
                let it = &self.items[i];
                let speed_str = fmt_speed(it.speed);
                state_changed
                    || it.st != it.last_pushed_st
                    || it.active != it.last_pushed_active
                    || speed_str != it.last_pushed_speed_fmt
                    || (it.done.saturating_sub(it.last_pushed_done) >= 65536)
                    || (it.total > 0 && ((it.done as f64 - it.last_pushed_done as f64) / (it.total as f64) >= 0.002))
                    || it.task.is_none()
            };
            if should_push {
                self.push_row(i);
            }
        }
        let total_speed = self.refresh_stats();
        self.sample_speed(total_speed);

        let notify = self.ui.upgrade().map(|u| u.get_notifications()).unwrap_or(false);
        for name in &finished {
            self.toast(&format!("Finished {name}"));
            if notify {
                sys::notify("Download complete", name);
            }
        }
        if changed || self.last_save.elapsed() > Duration::from_secs(5) {
            self.last_save = Instant::now();
            self.save_soon();
        }
        if changed {
            self.pump_queue();
        }
        if !self.items.iter().any(|i| i.task.is_some()) {
            self.tick.stop();
            self.speeds.clear();
            self.refresh_spark();
            // Window already closed and nothing left to do: finish quietly.
            if self.background && !self.busy() {
                if notify {
                    sys::notify("SDM finished", "All downloads are done.");
                }
                self.quit();
            }
        }
    }

    /// Pause everything and wait (briefly) for the engine to persist its state.
    fn shutdown(&mut self) {
        for it in &self.items {
            if let Some(t) = &it.task {
                t.cancel.cancel();
            }
        }
        let deadline = Instant::now() + Duration::from_millis(2000);
        while Instant::now() < deadline
            && self.items.iter().any(|i| i.task.as_ref().is_some_and(|t| t.state.load(Relaxed) == RUNNING))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        for it in &mut self.items {
            if let Some(t) = it.task.take() {
                it.done = t.downloaded.load(Relaxed);
                if t.state.load(Relaxed) == DONE {
                    it.st = St::Done;
                    it.finished = Some(now_secs());
                    if let Some(p) = t.path.lock().unwrap().clone() {
                        it.path = Some(p);
                    }
                    it.done = it.total.max(it.done);
                } else if it.st == St::Running {
                    it.st = St::Paused;
                }
            }
        }
        self.save_sync();
    }

    fn schedule_update_check(&mut self) {
        if !self.cfg.ipc {
            return;
        }
        if !crate::update::should_check_background(&self.cfg.data_dir) {
            return;
        }
        let me = self.me.clone();
        self.update_timer.start(TimerMode::SingleShot, Duration::from_secs(4), move || {
            with(&me, |s| s.check_update_silent());
        });
    }

    fn check_update_silent(&mut self) {
        crate::update::touch_update_check(&self.cfg.data_dir);
        self.rt.spawn(async move {
            if let Ok(info) = crate::update::check_for_update(None).await {
                if info.is_newer {
                    let _ = slint::invoke_from_event_loop(move || {
                        with_inner(|inner| {
                            inner.pending_update = Some(info.clone());
                            if let Some(ui) = inner.ui.upgrade() {
                                ui.set_update_state(2);
                                ui.set_update_status(format!("Update v{} is available!", info.latest_version).into());
                            }
                        });
                    });
                }
            }
        });
    }

    fn check_update(&mut self) {
        crate::update::touch_update_check(&self.cfg.data_dir);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_update_state(1);
            ui.set_update_status("Checking for updates…".into());
        }
        self.rt.spawn(async move {
            let res = crate::update::check_for_update(None).await;
            let _ = slint::invoke_from_event_loop(move || {
                with_inner(|inner| inner.on_update_checked(res));
            });
        });
    }

    fn on_update_checked(&mut self, res: Result<crate::update::UpdateInfo, String>) {
        let Some(ui) = self.ui.upgrade() else { return };
        match res {
            Ok(info) => {
                if info.is_newer {
                    self.pending_update = Some(info.clone());
                    ui.set_update_state(2);
                    ui.set_update_status(format!("Update v{} is available!", info.latest_version).into());
                    self.toast(&format!("SDM v{} is available!", info.latest_version));
                } else {
                    self.pending_update = None;
                    ui.set_update_state(0);
                    ui.set_update_status(format!("SDM is up to date (v{})", info.current_version).into());
                    self.toast("SDM is up to date");
                }
            }
            Err(err) => {
                self.pending_update = None;
                ui.set_update_state(5);
                ui.set_update_status(format!("Check failed: {err}").into());
                self.toast(&format!("Update check failed: {err}"));
            }
        }
    }

    fn apply_update(&mut self) {
        let Some(info) = self.pending_update.clone() else {
            self.check_update();
            return;
        };
        if let Err(msg) = crate::update::check_permissions() {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_update_state(5);
                ui.set_update_status(msg.clone().into());
            }
            self.toast(&msg);
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_update_state(3);
            ui.set_update_status(format!("Downloading v{}…", info.latest_version).into());
        }
        self.toast("Downloading update…");
        let ver = info.latest_version.clone();
        self.rt.spawn(async move {
            let res = crate::update::download_and_apply(&info).await;
            let _ = slint::invoke_from_event_loop(move || {
                with_inner(|inner| inner.on_update_applied(res, &ver));
            });
        });
    }

    fn on_update_applied(&mut self, res: Result<(), String>, version: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        match res {
            Ok(()) => {
                ui.set_update_state(4);
                ui.set_update_status(format!("Updated to v{}! Restart to apply.", version).into());
                self.toast(&format!("Updated to v{}! Restart to apply", version));
            }
            Err(err) => {
                ui.set_update_state(5);
                ui.set_update_status(format!("Update failed: {err}").into());
                self.toast(&format!("Update failed: {err}"));
            }
        }
    }

    fn restart_app(&mut self) {
        if let Err(e) = crate::update::restart_app() {
            self.toast(&format!("Restart failed: {e}"));
        }
    }
}

fn settings_from_ui(ui: &MainWindow, last_win_size: &Cell<(i32, i32)>) -> Settings {
    let size = ui.window().size();
    let f = ui.window().scale_factor().max(0.5);
    let (w, h) = ((size.width as f32 / f) as i32, (size.height as f32 / f) as i32);
    let (prev_w, prev_h) = last_win_size.get();
    let (window_width, window_height) = if w >= 760 && h >= 560 {
        last_win_size.set((w, h));
        (w, h)
    } else {
        (prev_w, prev_h)
    };
    Settings {
        download_folder: ui.get_download_folder().to_string(),
        connections: ui.get_default_conns(),
        max_active: ui.get_max_active(),
        speed_limit: ui.get_speed_limit(),
        theme: ui.global::<Theme>().get_mode(),
        clipboard: ui.get_clipboard_capture(),
        notifications: ui.get_notifications(),
        keep_running: ui.get_background_mode(),
        system_titlebar: ui.get_system_titlebar(),
        // A minimised/hidden window reports a tiny size: keep the last good one.
        window_width,
        window_height,
    }
}

/// Runs on the IPC thread: hand the request to the UI thread and wait for its answer.
fn dispatch_to_ui(req: ipc::Request) -> ipc::Reply {
    let (tx, rx) = std::sync::mpsc::channel();
    let queued = slint::invoke_from_event_loop(move || {
        let state = INNER.with(|c| c.borrow().as_ref().and_then(|w| w.upgrade()));
        let reply = match state {
            Some(rc) => match rc.try_borrow_mut() {
                Ok(mut g) => g.handle_ipc(req),
                Err(_) => ipc::Reply::err("SDM is busy, try again"),
            },
            None => ipc::Reply::err("SDM is shutting down"),
        };
        let _ = tx.send(reply);
    });
    if queued.is_err() {
        return ipc::Reply::err("SDM is shutting down");
    }
    rx.recv_timeout(Duration::from_secs(8)).unwrap_or_else(|_| ipc::Reply::err("SDM did not answer in time"))
}

fn with<R>(me: &RcWeak<RefCell<Inner>>, f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
    let rc = me.upgrade()?;
    let mut g = rc.try_borrow_mut().ok()?;
    Some(f(&mut g))
}

fn with_inner<R>(f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
    let rc = INNER.with(|c| c.borrow().as_ref().and_then(|w| w.upgrade()))?;
    let mut g = rc.try_borrow_mut().ok()?;
    Some(f(&mut g))
}

impl App {
    pub fn install(ui: &MainWindow, cfg: Config) -> App {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(48)
            .enable_all()
            .build()
            .expect("tokio runtime");

        // ---- settings -> UI (before any callbacks are connected) ----------
        let s = store::load_settings(&cfg.data_dir);
        ui.set_download_folder(s.download_folder.clone().into());
        ui.set_default_conns(s.connections.clamp(1, 32));
        ui.set_max_active(s.max_active.clamp(1, 8));
        ui.set_speed_limit(s.speed_limit.clamp(0, SPEED_LIMITS.len() as i32 - 1));
        ui.set_clipboard_capture(s.clipboard);
        ui.set_notifications(s.notifications);
        ui.set_background_mode(s.keep_running);
        ui.set_system_titlebar(s.system_titlebar);
        // SDM draws its own header bar unless asked not to (setting, or SDM_SYSTEM_TITLEBAR=1
        // as an escape hatch should a compositor misbehave).
        ui.set_custom_frame(!(s.system_titlebar || std::env::var_os("SDM_SYSTEM_TITLEBAR").is_some()));
        ui.set_version(env!("CARGO_PKG_VERSION").into());
        if s.window_width >= 760 && s.window_height >= 560 {
            ui.window().set_size(slint::LogicalSize::new(s.window_width as f32, s.window_height as f32));
        }
        ui.global::<Theme>().set_mode(s.theme.clamp(0, 2));

        // ---- history ----------------------------------------------------------
        let saved = store::load_history(&cfg.data_dir);
        let items: Vec<Item> = saved
            .iter()
            .map(|s| Item {
                id: s.id,
                url: s.url.clone(),
                name: s.name.clone(),
                dir: PathBuf::from(&s.dir),
                path: s.path.as_ref().map(PathBuf::from),
                total: s.total,
                done: s.done,
                conns: s.connections,
                st: if s.complete { St::Done } else { St::Paused },
                pausing: false,
                resume_after: false,
                speed: 0.0,
                last_done: s.done,
                active: 0,
                err: String::new(),
                finished: s.finished,
                task: None,
                headers: s.headers.clone(),
                last_pushed_done: s.done,
                last_pushed_speed_fmt: String::new(),
                last_pushed_st: if s.complete { St::Done } else { St::Paused },
                last_pushed_active: 0,
            })
            .collect();
        let model = Rc::new(VecModel::from(items.iter().map(to_row).collect::<Vec<_>>()));
        // The sidebar filters (All / Active / Completed / Stopped) are a view on one model.
        let page = Rc::new(std::cell::Cell::new(0i32));
        let filtered = Rc::new(slint::FilterModel::new(model.clone(), {
            let page = page.clone();
            move |d: &DownloadItem| match page.get() {
                1 => matches!(d.status.as_str(), "downloading" | "queued"),
                2 => d.status == "done",
                3 => matches!(d.status.as_str(), "paused" | "error"),
                _ => true,
            }
        }));
        ui.set_downloads(ModelRc::from(filtered.clone()));
        ui.on_page_changed({
            let (page, filtered) = (page.clone(), filtered.clone());
            move |p| {
                page.set(p);
                filtered.reset();
            }
        });

        let last_win_size = Rc::new(Cell::new((s.window_width, s.window_height)));
        let data_dir = cfg.data_dir.clone();
        let ipc_enabled = cfg.ipc;
        let inner = Rc::new_cyclic(|me| {
            RefCell::new(Inner {
                ui: ui.as_weak(),
                rt: rt.handle().clone(),
                client: engine::client().expect("http client"),
                limiter: RateLimiter::new(SPEED_LIMITS[s.speed_limit.clamp(0, SPEED_LIMITS.len() as i32 - 1) as usize]),
                next_id: items.iter().map(|i| i.id).max().unwrap_or(0) + 1,
                items,
                model,
                cfg,
                tick: Timer::default(),
                save: Timer::default(),
                update_timer: Timer::default(),
                last_tick: Instant::now(),
                last_save: Instant::now(),
                background: false,
                speeds: VecDeque::new(),
                last_sample: Instant::now(),
                me: me.clone(),
                pending_update: None,
                last_win_size: last_win_size.clone(),
            })
        });
        inner.borrow().refresh_stats();
        inner.borrow_mut().schedule_update_check();
        let me = Rc::downgrade(&inner);
        INNER.with(|c| *c.borrow_mut() = Some(me.clone()));
        ui.set_ext_state(if ipc::install::is_installed() { 1 } else { 0 });

        // Listen for the browser extension / `sdm <url>` from a second launch.
        let ipc_server = if ipc_enabled {
            let handler: ipc::Handler = Arc::new(dispatch_to_ui);
            ipc::Server::start(&data_dir, env!("CARGO_PKG_VERSION"), handler).ok()
        } else {
            None
        };

        // ---- UI callbacks -------------------------------------------------
        ui.on_add_quick({
            let (me, w) = (me.clone(), ui.as_weak());
            move |url| {
                if with(&me, |s| s.add(&url, None, None)) == Some(true) {
                    if let Some(ui) = w.upgrade() {
                        ui.set_link_text(SharedString::new());
                    }
                }
            }
        });
        ui.on_add_advanced({
            let me = me.clone();
            move |url, folder, conns| {
                with(&me, |s| s.add(&url, Some(PathBuf::from(folder.as_str())), Some(conns)));
            }
        });
        ui.on_setup_extension({
            let me = me.clone();
            move || {
                with(&me, |s| s.setup_extension());
            }
        });
        ui.on_limits_changed({
            let me = me.clone();
            move || {
                with(&me, |s| s.apply_limits());
            }
        });
        ui.on_paste_requested({
            let me = me.clone();
            move || {
                with(&me, |s| s.paste());
            }
        });
        ui.on_toggle({
            let me = me.clone();
            move |id| {
                with(&me, |s| s.toggle(id));
            }
        });
        ui.on_retry({
            let me = me.clone();
            move |id| {
                with(&me, |s| s.toggle(id));
            }
        });
        ui.on_remove({
            let me = me.clone();
            move |id| {
                with(&me, |s| s.remove(id));
            }
        });
        ui.on_open_folder({
            let me = me.clone();
            move |id| {
                with(&me, |s| s.open_folder(id));
            }
        });
        ui.on_open_file({
            let me = me.clone();
            move |id| {
                with(&me, |s| s.open_file(id));
            }
        });
        ui.on_copy_link({
            let me = me.clone();
            move |id| {
                with(&me, |s| s.copy_link(id));
            }
        });
        ui.on_browse_folder({
            let (w, handle) = (ui.as_weak(), rt.handle().clone());
            move || {
                let Some(ui) = w.upgrade() else { return };
                let start = PathBuf::from(ui.get_download_folder().as_str());
                let w2 = w.clone();
                handle.spawn(async move {
                    let pick = rfd::AsyncFileDialog::new().set_title("Choose download folder").set_directory(start).pick_folder().await;
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = w2.upgrade() else { return };
                        match pick {
                            Some(f) => ui.set_download_folder(f.path().to_string_lossy().into_owned().into()),
                            None => {}
                        }
                    });
                });
            }
        });

        // Settings are saved a moment after the last change.
        let settings_timer = Rc::new(Timer::default());
        ui.on_settings_changed({
            let (w, t, dir, win_sz) = (ui.as_weak(), settings_timer.clone(), data_dir.clone(), last_win_size.clone());
            move || {
                let (w, dir, win_sz) = (w.clone(), dir.clone(), win_sz.clone());
                t.start(TimerMode::SingleShot, Duration::from_millis(400), move || {
                    let Some(ui) = w.upgrade() else { return };
                    store::save_settings(&dir, &settings_from_ui(&ui, &win_sz));
                });
            }
        });

        // Window management for SDM's own header bar.
        ui.on_begin_drag({
            let w = ui.as_weak();
            move || {
                if let Some(ui) = w.upgrade() {
                    ui.window().with_winit_window(|win| {
                        let _ = win.drag_window();
                    });
                }
            }
        });
        ui.on_begin_resize({
            let w = ui.as_weak();
            move |dir| {
                use winit::window::ResizeDirection as D;
                let d = match dir.as_str() {
                    "n" => D::North,
                    "s" => D::South,
                    "e" => D::East,
                    "w" => D::West,
                    "ne" => D::NorthEast,
                    "nw" => D::NorthWest,
                    "se" => D::SouthEast,
                    _ => D::SouthWest,
                };
                if let Some(ui) = w.upgrade() {
                    ui.window().with_winit_window(|win| {
                        let _ = win.drag_resize_window(d);
                    });
                }
            }
        });
        ui.on_toggle_maximize({
            let w = ui.as_weak();
            move || {
                if let Some(ui) = w.upgrade() {
                    ui.window().with_winit_window(|win| win.set_maximized(!win.is_maximized()));
                }
            }
        });
        ui.on_close_window({
            let w = ui.as_weak();
            move || {
                if let Some(ui) = w.upgrade() {
                    ui.window().dispatch_event(slint::platform::WindowEvent::CloseRequested);
                }
            }
        });
        ui.on_frame_changed({
            let w = ui.as_weak();
            move || {
                if let Some(ui) = w.upgrade() {
                    // Maximised / fullscreen windows have square corners and no edge grips.
                    let squared = ui.window().with_winit_window(|win| win.is_maximized() || win.fullscreen().is_some()).unwrap_or(false);
                    if ui.get_squared() != squared {
                        ui.set_squared(squared);
                    }
                }
            }
        });

        // Closing the window: keep going in the background, or pause everything and quit.
        ui.window().on_close_requested({
            let me = me.clone();
            move || {
                with(&me, |s| s.on_window_close());
                CloseRequestResponse::HideWindow
            }
        });
        ui.on_quit_requested({
            let me = me.clone();
            move || {
                with(&me, |s| s.quit());
            }
        });
        ui.on_pause_all({
            let me = me.clone();
            move || {
                with(&me, |s| s.pause_all());
            }
        });
        ui.on_resume_all({
            let me = me.clone();
            move || {
                with(&me, |s| s.resume_all());
            }
        });
        ui.on_clear_done({
            let me = me.clone();
            move || {
                with(&me, |s| s.clear_done());
            }
        });
        ui.on_open_download_folder({
            let me = me.clone();
            move || {
                with(&me, |s| s.open_download_folder());
            }
        });
        ui.on_check_update({
            let me = me.clone();
            move || {
                with(&me, |s| s.check_update());
            }
        });
        ui.on_apply_update({
            let me = me.clone();
            move || {
                with(&me, |s| s.apply_update());
            }
        });
        ui.on_restart_app({
            let me = me.clone();
            move || {
                with(&me, |s| s.restart_app());
            }
        });

        // Keep "Finished 4 minutes ago" fresh.
        let slow = Timer::default();
        slow.start(TimerMode::Repeated, Duration::from_secs(30), {
            let me = me.clone();
            move || {
                with(&me, |s| {
                    for i in 0..s.items.len() {
                        if s.items[i].st == St::Done {
                            s.push_row(i);
                        }
                    }
                });
            }
        });

        App { inner, _slow: slow, _settings_timer: settings_timer, _ipc: ipc_server, _rt: rt }
    }

    /// Start a download from outside the UI (command line, browser extension).
    pub fn add_url(&self, url: &str) {
        if let Ok(mut g) = self.inner.try_borrow_mut() {
            g.add(url, None, None);
        }
    }
}
