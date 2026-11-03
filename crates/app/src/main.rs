use slint::ComponentHandle;
use std::time::Duration;
use sdm_app::{App, Config, MainWindow};

fn main() -> Result<(), slint::PlatformError> {
    if std::env::args().any(|a| a == "--update" || a == "-U") {
        if let Err(e) = sdm_app::update::run_cli() {
            eprintln!("Update error: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    if std::env::args().any(|a| a == "--version" || a == "-v" || a == "-V") {
        println!("sdm {}", sdm_app::update::current_version());
        return Ok(());
    }

    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "SDM - Smart Download Manager\n\n\
            Usage:\n  \
              sdm [OPTIONS] [URL]\n\n\
            Arguments:\n  \
              [URL]          Download link to capture and start\n\n\
            Options:\n  \
              -U, --update   Check for updates and apply them directly\n  \
              -v, --version  Print version information\n  \
              -h, --help     Print help information"
        );
        return Ok(());
    }

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
