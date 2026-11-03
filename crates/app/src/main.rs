use slint::ComponentHandle;
use std::time::Duration;
use sdm_app::{App, Config, MainWindow};

fn main() -> Result<(), slint::PlatformError> {
    let url = std::env::args().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://"));
    let data_dir = Config::from_system().data_dir;

    // Already running? Hand the link (or a "bring to front") over and quit.
    let mut req = ipc::Request::new(if url.is_some() { "add" } else { "show" });
    req.source = Some("cli".into());
    if let Some(u) = &url {
        req.url = u.clone();
    }
    if let Ok(reply) = ipc::client::send(&data_dir, req, Duration::from_secs(5)) {
        if reply.ok {
            return Ok(());
        }
    }

    let _ = slint::set_xdg_app_id("sdm");
    let ui = MainWindow::new()?;
    let app = App::install(&ui, Config::from_system());
    if let Some(u) = url {
        app.add_url(&u);
    }
    ui.show()?;
    // Keeps running while the window is hidden so downloads can continue in the background.
    slint::run_event_loop_until_quit()?;
    drop(app);
    Ok(())
}
