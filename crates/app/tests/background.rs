//! Closing the window, background downloads, bulk actions and the speed sparkline.
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model};
use std::sync::atomic::Ordering;
use sdm_app::devserver::{expected, serve};
use sdm_app::{store, App, Config, MainWindow};

mod common;
use common::*;

fn start(data: &std::path::Path, dl: &std::path::Path) -> (MainWindow, App) {
    let ui = MainWindow::new().unwrap();
    let app = App::install(&ui, Config { data_dir: data.to_path_buf(), ipc: false });
    ui.show().unwrap();
    ui.set_download_folder(dl.to_string_lossy().into_owned().into());
    ui.set_default_conns(2);
    (ui, app)
}

#[test]
fn background_downloads_bulk_actions_and_sparkline() {
    let win = setup();
    let data = tmp("bg-data");
    let dl = tmp("bg-dl");
    let slow = serve(4_000_000, 2_000_000); // 2 conns x 2 MB/s ~ 1 s per file
    let slower = serve(4_000_000, 1_000_000); // ~2 s per file

    // 1) Closing the window while downloading keeps the app alive; it quits by itself when done.
    {
        let (ui, app) = start(&data, &dl);
        ui.set_background_mode(true);
        ui.invoke_add_quick(format!("{slower}/files/a.bin").into());
        pump_until(10, "progress", || progress(&ui, 0) > 0.1);
        QUIT.store(false, Ordering::SeqCst);
        win.dispatch_event(WindowEvent::CloseRequested);
        pump_for(150);
        assert!(!QUIT.load(Ordering::SeqCst), "must keep running in the background");
        assert_eq!(status(&ui, 0), "downloading");
        pump_until(5, "speed sparkline", || !ui.get_speed_path().is_empty());
        assert!(ui.get_speed_fill().to_string().ends_with('Z'));
        pump_until(20, "app quits once the download is finished", || QUIT.load(Ordering::SeqCst));
        assert_eq!(std::fs::read(dl.join("a.bin")).unwrap(), expected(4_000_000));
        let s = store::load_settings(&data);
        assert_eq!((s.window_width, s.window_height), (1040, 720), "window size remembered");
        drop(app);
        ui.hide().unwrap();
    }

    // 2) Nothing running: closing the window quits right away. History is restored.
    {
        QUIT.store(false, Ordering::SeqCst);
        let (ui, app) = start(&data, &dl);
        assert_eq!(ui.get_downloads().row_count(), 1);
        assert_eq!(status(&ui, 0), "done");
        win.dispatch_event(WindowEvent::CloseRequested);
        pump_for(100);
        assert!(QUIT.load(Ordering::SeqCst));
        drop(app);
        ui.hide().unwrap();
    }

    // 3) Background mode off: closing pauses the download and quits immediately.
    {
        QUIT.store(false, Ordering::SeqCst);
        let (ui, app) = start(&data, &dl);
        ui.set_background_mode(false);
        ui.invoke_add_quick(format!("{slow}/files/b.bin").into());
        pump_until(10, "progress", || progress(&ui, 0) > 0.1);
        win.dispatch_event(WindowEvent::CloseRequested);
        pump_for(100);
        assert!(QUIT.load(Ordering::SeqCst));
        let h = store::load_history(&data);
        let b = h.iter().find(|x| x.name == "b.bin").expect("b.bin saved");
        assert!(!b.complete && b.done > 0, "paused with progress kept");
        drop(app);
        ui.hide().unwrap();
    }

    // 4) Bulk actions: pause all, resume all (restores the paused b.bin too), clear completed.
    {
        QUIT.store(false, Ordering::SeqCst);
        let (ui, _app) = start(&data, &dl);
        ui.set_max_active(4);

        // Sidebar filters are views on the same list (restored: a.bin done, b.bin paused).
        let rows = |page: i32| {
            ui.set_page(page);
            pump_for(60);
            (0..ui.get_downloads().row_count()).map(|i| status(&ui, i)).collect::<Vec<_>>()
        };
        assert_eq!(rows(0).len(), 2);
        assert_eq!(rows(2), vec!["done"]);
        assert_eq!(rows(3), vec!["paused"]);
        assert!(rows(1).is_empty());
        assert_eq!((ui.get_count_all(), ui.get_done_count(), ui.get_count_stopped(), ui.get_count_busy()), (2, 1, 1, 0));
        assert_eq!(rows(0).len(), 2);

        for n in ["c", "d", "e"] {
            ui.invoke_add_quick(format!("{slow}/files/{n}.bin").into());
        }
        pump_until(10, "all three running", || (0..3).all(|i| progress(&ui, i) > 0.05));
        ui.invoke_pause_all();
        pump_until(10, "all paused", || (0..3).all(|i| status(&ui, i) == "paused"));
        ui.invoke_resume_all();
        pump_until(40, "everything finished", || (0..ui.get_downloads().row_count()).all(|i| status(&ui, i) == "done"));
        for n in ["a", "b", "c", "d", "e"] {
            assert_eq!(std::fs::read(dl.join(format!("{n}.bin"))).unwrap(), expected(4_000_000), "{n}.bin");
        }
        pump_until(5, "sparkline cleared when idle", || ui.get_speed_path().is_empty());
        ui.invoke_clear_done();
        assert_eq!(ui.get_downloads().row_count(), 0);
        assert!(dl.join("a.bin").exists(), "clearing the list keeps the files");
    }

    // 5) Damaged state files never stop the app from starting.
    {
        let data2 = tmp("bg-corrupt");
        std::fs::write(data2.join("downloads.json"), b"[{\"id\": 1, \"url\": ").unwrap();
        std::fs::write(data2.join("settings.json"), b"\x00\x01 not json").unwrap();
        let ui = MainWindow::new().unwrap();
        let _app = App::install(&ui, Config { data_dir: data2.clone(), ipc: false });
        assert_eq!(ui.get_downloads().row_count(), 0);
        assert_eq!(ui.get_default_conns(), 16, "falls back to defaults");
        // and a newly added download works normally afterwards
        ui.show().unwrap();
        ui.set_download_folder(dl.to_string_lossy().into_owned().into());
        ui.invoke_add_quick(format!("{slow}/files/after-corruption.bin").into());
        pump_until(20, "download after corrupt state", || status(&ui, 0) == "done");
    }
}
