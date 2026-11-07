# SDM - architecture (for whoever works on it next, human or AI)

Rust workspace, five crates, one browser extension. Everything is tested headlessly.

```
crates/engine   the download engine (no UI, no globals)          tests: crates/engine/tests/single.rs
crates/app      Slint UI + glue ("sdm" binary, lib "sdm_app")   tests: crates/app/tests/*.rs
crates/ipc      app <-> host <-> CLI control channel + installer   tests: crates/ipc/tests/*.rs
crates/host     "sdm-host": native-messaging helper browsers start tests: crates/host/tests/host.rs
crates/cli      "sdm-cli": engine from a terminal (benchmarking)
extension/      Chrome/Edge/Brave (MV3) and Firefox (MV2) extension tests: extension/test/*.js (node --test)
packaging/      .desktop, icons, install.sh, RPM spec
scripts/        bench.sh, rangeserver.py, pack-extension.sh
```

## engine (`crates/engine`)
Entry point: `engine::download(client, url, dir, &Options, &CancellationToken, on_progress)`.

1. `probe` sends `GET Range: bytes=0-0` (HEAD is unreliable): size, range support, ETag, file name. 416 = empty file.
2. `locate` picks the target path (resumes a matching `.part.json`, else `name (N).ext`).
3. Multi-connection path (`multi.rs`) when the server supports ranges and the file is >= 1 MiB:
   - the file is preallocated; each worker owns a byte range `Seg` and writes with positional writes (`pwrite`) from a 512 KiB buffer (flushed at least every 250 ms),
   - HTTP/1.1 only, so every worker is its own TCP connection,
   - dynamic splitting: an idle worker steals half of the largest remaining segment (`Ctx::steal`),
   - `Busy` errors (429/503/connect failures) make workers give their segment back and exit (`try_shrink`), and `Retry-After` is honoured,
   - state (`.part.json`: URL, size, ETag, remaining ranges) is saved every second and on pause,
   - a stale ETag/Last-Modified/size throws the old partial data away.
4. Fallback single-connection path (`single.rs`): unknown size, no ranges, tiny files.
5. `RateLimiter` (shared, bytes/s) can cap all downloads together. `Options.headers` carries cookies/referer.

Cancelling the token = pause (state is saved, `Outcome::Paused`). Any error leaves the partial data so a retry resumes.

## app (`crates/app`)
- `ui/*.slint`: all visuals, modelled on GNOME's libadwaita (flat colours, boxed lists, sidebar). `theme.slint` holds the palette and follows the system light/dark setting, `components.slint` the widgets, `row.slint` a download row, `pages.slint` Preferences and Browser Extension, `overlays.slint` the New Download dialog and toast, `app.slint` the window. The sidebar filters are a `FilterModel` over one list (see `App::install`). Icons are SVGs in `ui/icons`, recoloured at runtime.
- `src/app.rs`: the only glue. Downloads run on a 2-thread tokio runtime and publish progress into atomics (`Task`). A UI-thread timer (running only while something downloads) turns them into list rows. Queue (`St::Queued`), pause/resume, background mode, settings, history persistence (`store.rs`), clipboard (through a hidden Slint `TextInput`, which uses the window's own clipboard, so it works on Wayland).
- IPC requests arrive on another thread and are executed on the UI thread through `slint::invoke_from_event_loop` (`dispatch_to_ui`).
- `src/devserver.rs` is a tiny range server used by tests and the screenshot tool.
- Screenshot tool: `cargo run -p sdm-app --example shot -- /tmp/shots` renders PNGs with the software renderer (no display needed).

State lives in `~/.config/sdm` (`settings.json`, `downloads.json` - may hold cookies, mode 0600 - `ipc.json`, `host.log`). `SDM_DATA_DIR` overrides it.

## browser integration
Extension (`extension/common.js`) -> `chrome.runtime.sendNativeMessage("com.sdm.host")` -> `sdm-host` (stdin/stdout, 4-byte length + JSON) -> TCP on 127.0.0.1 with a secret token from `ipc.json` -> app. The browser's own download is cancelled only after SDM confirmed. `sdm-host` starts the app if it is not running. `ipc::install` writes the native-messaging manifests and unpacks the extension (`sdm-host --install` does the same from a terminal).

## Testing recipes
- Everything: `cargo test --workspace && (cd extension && node --test test/common.test.js)`
- Add a feature to the engine: write the test first in `crates/engine/tests/single.rs`; its in-process server can throttle, drop connections, reject with 503, require headers, lie about the size, go chunked.
- UI behaviour: `crates/app/tests/common/mod.rs` provides a headless Slint platform (`setup`, `pump_until`, ...). Drive the UI with `ui.invoke_*`, `ui.set_*`, and key events via `win.dispatch_event`.
- Look at the UI: the `shot` example above.
