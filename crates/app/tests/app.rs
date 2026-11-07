//! End-to-end: real UI + real engine + local HTTP server, headless.
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model};
use std::path::Path;
use sdm_app::devserver::{expected, serve};
use sdm_app::{store, App, Config, MainWindow};

mod common;
use common::*;

fn ctrl_v(win: &MinimalSoftwareWindow) {
    win.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    win.dispatch_event(WindowEvent::KeyPressed { text: "v".into() });
    win.dispatch_event(WindowEvent::KeyReleased { text: "v".into() });
    win.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn full_flow() {
    let win = setup();
    let data = tmp("app-data");
    let dl = tmp("app-dl");
    let fast = serve(12_000_000, 0);
    let slow = serve(4_000_000, 2_000_000); // 2 MB/s per connection

    // ---------------- first run -------------------------------------------
    {
        let ui = MainWindow::new().unwrap();
        let app = App::install(&ui, Config { data_dir: data.clone(), ipc: false });
        ui.show().unwrap();
        win.dispatch_event(WindowEvent::WindowActiveChanged(true));
        ui.set_download_folder(dl.to_string_lossy().into_owned().into());
        pump_for(50);

        // Ctrl+V with a link buried in other text captures and downloads it
        CLIP.with(|c| *c.borrow_mut() = Some(format!("look at this\n  {fast}/files/data.bin  \nthanks")));
        ctrl_v(&win);
        pump_until(2, "row to appear", || ui.get_downloads().row_count() == 1);
        assert_eq!(ui.get_downloads().row_data(0).unwrap().name, "data.bin");
        pump_until(30, "download to finish", || status(&ui, 0) == "done");
        assert_eq!(std::fs::read(dl.join("data.bin")).unwrap(), expected(12_000_000));
        assert_eq!(ui.get_done_count(), 1);
        assert_eq!(files_in(&dl), vec!["data.bin"], "no .part leftovers");

        // clipboard without a link is rejected
        CLIP.with(|c| *c.borrow_mut() = Some("just words".into()));
        ctrl_v(&win);
        pump_for(100);
        assert_eq!(ui.get_downloads().row_count(), 1);

        // same link again does not create a duplicate while it is active;
        // but a finished one can be downloaded again as "data (1).bin"
        CLIP.with(|c| *c.borrow_mut() = Some(format!("{fast}/files/data.bin")));
        ctrl_v(&win);
        pump_until(30, "second copy", || ui.get_downloads().row_count() == 2 && status(&ui, 0) == "done");
        assert_eq!(files_in(&dl), vec!["data (1).bin", "data.bin"]);

        // copy-link puts the URL on the clipboard
        let id = ui.get_downloads().row_data(0).unwrap().id;
        ui.invoke_copy_link(id);
        assert_eq!(CLIP.with(|c| c.borrow().clone()).unwrap(), format!("{fast}/files/data.bin"));

        // pause / resume on a throttled server (2 connections ≈ 4 MB/s total)
        ui.set_default_conns(2);
        ui.invoke_add_quick(format!("{slow}/files/slow.bin").into());
        pump_until(10, "progress", || progress(&ui, 0) > 0.15);
        let id = ui.get_downloads().row_data(0).unwrap().id;
        ui.invoke_toggle(id);
        pump_until(5, "paused", || status(&ui, 0) == "paused");
        let at_pause = progress(&ui, 0);
        assert!(at_pause > 0.1 && at_pause < 1.0, "paused at {at_pause}");
        pump_for(300);
        assert_eq!(status(&ui, 0), "paused", "stays paused");
        ui.invoke_toggle(id);
        pump_until(20, "resumed download to finish", || status(&ui, 0) == "done");
        assert_eq!(std::fs::read(dl.join("slow.bin")).unwrap(), expected(4_000_000));

        // errors are shown in the row, and retry is available
        ui.invoke_add_quick(format!("{fast}/missing/nope.zip").into());
        pump_until(10, "error row", || status(&ui, 0) == "error");
        assert!(ui.get_downloads().row_data(0).unwrap().detail.contains("404"), "{}", ui.get_downloads().row_data(0).unwrap().detail);

        // removing a paused/unfinished download cleans its partial files
        ui.set_default_conns(2);
        ui.invoke_add_quick(format!("{slow}/files/partial.bin").into());
        pump_until(10, "progress", || progress(&ui, 0) > 0.1);
        let id = ui.get_downloads().row_data(0).unwrap().id;
        ui.invoke_remove(id);
        pump_for(1200);
        assert!(!files_in(&dl).iter().any(|f| f.starts_with("partial")), "{:?}", files_in(&dl));

        // queue: with one slot, the second download waits and starts by itself
        ui.set_max_active(1);
        ui.set_default_conns(2);
        ui.invoke_add_quick(format!("{slow}/files/q1.bin").into());
        ui.invoke_add_quick(format!("{slow}/files/q2.bin").into());
        pump_until(5, "q2 queued while q1 runs", || status(&ui, 0) == "queued" && status(&ui, 1) == "downloading");
        pump_until(40, "queue to drain", || status(&ui, 0) == "done" && status(&ui, 1) == "done");
        assert_eq!(std::fs::read(dl.join("q1.bin")).unwrap(), expected(4_000_000));
        assert_eq!(std::fs::read(dl.join("q2.bin")).unwrap(), expected(4_000_000));

        // speed limit from the UI applies immediately, and can be lifted again
        ui.set_max_active(4);
        ui.set_default_conns(8);
        ui.set_speed_limit(1); // 512 KB/s
        pump_for(100);
        ui.invoke_add_quick(format!("{fast}/files/limited.bin").into());
        pump_for(2000);
        let p = progress(&ui, 0);
        assert!(p > 0.0 && p < 0.25, "limited download progressed to {p}");
        ui.set_speed_limit(0);
        pump_until(30, "limit lifted, download finishes", || status(&ui, 0) == "done");
        assert_eq!(std::fs::read(dl.join("limited.bin")).unwrap(), expected(12_000_000));

        // settings persist
        ui.set_max_active(3);
        ui.set_default_conns(7);
        ui.set_clipboard_capture(false);
        ui.global::<sdm_app::Theme>().set_mode(1);
        ui.invoke_show_toast("x".into());
        // settings-changed is raised by the UI toggle/handlers; call it like they do
        ui.set_notifications(false);
        pump_for(1200);
        let s = store::load_settings(&data);
        assert_eq!(s.connections, 7);
        assert_eq!(s.max_active, 3);
        assert!(!s.clipboard && !s.notifications);

        drop(app);
        ui.hide().unwrap();
        pump_for(700);
    }

    // ---------------- second run: history is restored ------------------------
    {
        let hist = store::load_history(&data);
        assert!(hist.iter().any(|h| h.name == "data.bin" && h.complete));
        let ui = MainWindow::new().unwrap();
        let _app = App::install(&ui, Config { data_dir: data.clone(), ipc: false });
        ui.show().unwrap();
        let names: Vec<String> = ui.get_downloads().iter().map(|d| d.name.to_string()).collect();
        assert!(names.contains(&"data.bin".to_string()), "{names:?}");
        assert!(names.contains(&"slow.bin".to_string()));
        assert_eq!(ui.get_default_conns(), 7);
        assert_eq!(ui.get_max_active(), 3);
        assert!(!ui.get_clipboard_capture());
        assert_eq!(ui.global::<sdm_app::Theme>().get_mode(), 1);
        let first = ui.get_downloads().row_data(0).unwrap();
        assert!(first.status == "done" || first.status == "paused");
    }
}
