use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Shared bandwidth limiter (bytes per second, 0 = unlimited). One instance can
/// be shared by every connection of every download to enforce a global cap.
pub struct RateLimiter {
    rate: AtomicU64,
    next: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Arc<Self> {
        Arc::new(Self { rate: AtomicU64::new(bytes_per_sec), next: Mutex::new(Instant::now()) })
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Relaxed)
    }

    /// Wait until `n` more bytes may be processed.
    pub async fn acquire(&self, n: usize) {
        let rate = self.rate.load(Relaxed);
        if rate == 0 {
            return;
        }
        let wait = {
            let mut next = self.next.lock().unwrap();
            let now = Instant::now();
            let start = (*next).max(now);
            *next = start + Duration::from_secs_f64(n as f64 / rate as f64);
            start - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}
