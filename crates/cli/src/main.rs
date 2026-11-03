use anyhow::Result;
use clap::Parser;
use engine::{CancellationToken, Options, Outcome, RateLimiter};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "sdm", about = "SDM download manager (CLI test harness)")]
struct Args {
    url: String,
    /// Download folder (default: your Downloads folder)
    #[arg(short, long)]
    dir: Option<PathBuf>,
    /// Parallel connections, 1-32
    #[arg(short = 'c', long, default_value_t = 8)]
    connections: usize,
    /// Delete the file when finished (for speed benchmarks)
    #[arg(long)]
    discard: bool,
    /// Speed limit in MB/s (0 = unlimited)
    #[arg(long, default_value_t = 0.0)]
    limit: f64,
    /// Extra request header, e.g. -H "Cookie: a=b" (repeatable)
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,
}

fn human(b: f64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let (mut v, mut i) = (b, 0);
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let dir = args.dir.or_else(dirs::download_dir).unwrap_or_else(|| ".".into());
    let client = engine::client()?;
    let headers = args
        .headers
        .iter()
        .filter_map(|h| h.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    let limiter = (args.limit > 0.0).then(|| RateLimiter::new((args.limit * 1_048_576.0) as u64));
    let opts = Options { connections: args.connections, headers, limiter, ..Default::default() };

    let token = CancellationToken::new();
    {
        let t = token.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            t.cancel();
            let _ = tokio::signal::ctrl_c().await; // second Ctrl+C: force quit
            std::process::exit(130);
        });
    }

    // Speed is measured from the first progress event of THIS run, so bytes
    // already on disk from a previous run don't inflate it.
    let mut base: Option<(Instant, u64)> = None;
    let mut last = Instant::now() - Duration::from_secs(1);
    let res = engine::download(&client, &args.url, &dir, &opts, &token, |p| {
        let (t0, b0) = *base.get_or_insert((Instant::now(), p.downloaded));
        if last.elapsed() < Duration::from_millis(200) {
            return;
        }
        last = Instant::now();
        let speed = p.downloaded.saturating_sub(b0) as f64 / t0.elapsed().as_secs_f64().max(0.001);
        let pct = p.total.map(|t| format!("{:5.1}%", p.downloaded as f64 * 100.0 / t as f64)).unwrap_or_default();
        print!("\r{pct}  {}  {}/s  [{} conn]   ", human(p.downloaded as f64), human(speed), p.connections);
        let _ = std::io::stdout().flush();
    })
    .await;

    match res {
        Ok(Outcome::Done(path)) => {
            let (t0, b0) = base.unwrap_or((Instant::now(), 0));
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let secs = t0.elapsed().as_secs_f64().max(0.001);
            let avg = size.saturating_sub(b0) as f64 / secs;
            println!("\nDone: {}  ({} in {:.1}s, avg {}/s)", path.display(), human(size as f64), secs, human(avg));
            if args.discard {
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(Outcome::Paused) => println!("\nPaused. Run the same command again to resume."),
        Err(e) => {
            eprintln!("\nError: {e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}
