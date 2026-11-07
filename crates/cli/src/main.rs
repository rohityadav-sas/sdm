use anyhow::{bail, Context, Result};
use engine::{fmt_size, CancellationToken, Options, Outcome, RateLimiter};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Args {
    url: String,
    dir: Option<PathBuf>,
    connections: usize,
    discard: bool,
    limit: f64,
    headers: Vec<String>,
}

fn parse_args() -> Result<Args> {
    let mut url = None;
    let mut dir = None;
    let mut connections = 8usize;
    let mut discard = false;
    let mut limit = 0.0f64;
    let mut headers = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-d" | "--dir" => dir = Some(PathBuf::from(it.next().context("missing value for --dir")?)),
            "-c" | "--connections" => {
                connections = it.next().context("missing value for --connections")?.parse()?
            }
            "--discard" => discard = true,
            "--limit" => limit = it.next().context("missing value for --limit")?.parse()?,
            "-H" | "--header" => headers.push(it.next().context("missing value for --header")?),
            "-h" | "--help" => {
                println!("Usage: sdm-cli [OPTIONS] <URL>\n  -d, --dir <DIR>\n  -c, --connections <N>\n      --discard\n      --limit <MB/s>\n  -H, --header <HEADER>");
                std::process::exit(0);
            }
            s if !s.starts_with('-') && url.is_none() => url = Some(s.to_string()),
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { url: url.context("missing <URL>")?, dir, connections, discard, limit, headers })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
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
        let speed = (p.downloaded.saturating_sub(b0) as f64 / t0.elapsed().as_secs_f64().max(0.001)) as u64;
        let pct = p.total.map(|t| format!("{:5.1}%", p.downloaded as f64 * 100.0 / t as f64)).unwrap_or_default();
        print!("\r{pct}  {}  {}/s  [{} conn]   ", fmt_size(p.downloaded), fmt_size(speed), p.connections);
        let _ = std::io::stdout().flush();
    })
    .await;

    match res {
        Ok(Outcome::Done(path)) => {
            let (t0, b0) = base.unwrap_or((Instant::now(), 0));
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let secs = t0.elapsed().as_secs_f64().max(0.001);
            let avg = (size.saturating_sub(b0) as f64 / secs) as u64;
            println!("\nDone: {}  ({} in {:.1}s, avg {}/s)", path.display(), fmt_size(size), secs, fmt_size(avg));
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
