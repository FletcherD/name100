//! Per-client rate limiting: one token bucket per address.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Buckets kept before idle ones are swept out.
const SWEEP_AT: usize = 10_000;

struct Bucket {
    tokens: f64,
    updated: Instant,
}

pub struct RateLimiter {
    /// Tokens added per second.
    refill: f64,
    /// Most tokens a bucket holds: the burst a client may spend at once.
    burst: f64,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    /// A limiter allowing `per_minute` requests per address, with bursts of
    /// a tenth of that (at least 5). `None` when `per_minute` is 0.
    pub fn per_minute(per_minute: u32) -> Option<RateLimiter> {
        (per_minute > 0).then(|| RateLimiter {
            refill: per_minute as f64 / 60.0,
            burst: (per_minute as f64 / 10.0).max(5.0),
            buckets: Mutex::new(HashMap::new()),
        })
    }

    /// Spend one token for `client`, or say how long until one is available.
    pub fn check(&self, client: IpAddr) -> Result<(), Duration> {
        self.check_at(client, Instant::now())
    }

    fn check_at(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if buckets.len() >= SWEEP_AT {
            // A bucket idle long enough to be full again carries no information.
            let full_after = Duration::from_secs_f64(self.burst / self.refill);
            buckets.retain(|_, bucket| now.saturating_duration_since(bucket.updated) < full_after);
        }
        let bucket = buckets.entry(client).or_insert(Bucket { tokens: self.burst, updated: now });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill).min(self.burst);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - bucket.tokens) / self.refill))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn zero_means_no_limit() {
        assert!(RateLimiter::per_minute(0).is_none());
    }

    #[test]
    fn a_burst_is_allowed_then_requests_are_refused_until_tokens_return() {
        // 60 per minute: a burst of 6, then one per second.
        let limiter = RateLimiter::per_minute(60).unwrap();
        let start = Instant::now();
        for _ in 0..6 {
            assert!(limiter.check_at(address(1), start).is_ok());
        }
        let wait = limiter.check_at(address(1), start).unwrap_err();
        assert!((wait.as_secs_f64() - 1.0).abs() < 0.01, "{wait:?}");

        assert!(limiter.check_at(address(1), start + Duration::from_millis(500)).is_err());
        assert!(limiter.check_at(address(1), start + Duration::from_millis(1100)).is_ok());
        assert!(limiter.check_at(address(1), start + Duration::from_millis(1200)).is_err());
    }

    #[test]
    fn clients_are_limited_separately() {
        let limiter = RateLimiter::per_minute(60).unwrap();
        let now = Instant::now();
        while limiter.check_at(address(1), now).is_ok() {}
        assert!(limiter.check_at(address(2), now).is_ok());
    }

    #[test]
    fn an_idle_client_gets_at_most_one_burst() {
        let limiter = RateLimiter::per_minute(60).unwrap();
        let start = Instant::now();
        while limiter.check_at(address(1), start).is_ok() {}
        let later = start + Duration::from_secs(3600);
        let allowed = (0..100).filter(|_| limiter.check_at(address(1), later).is_ok()).count();
        assert_eq!(allowed, 6);
    }

    #[test]
    fn idle_buckets_are_swept() {
        let limiter = RateLimiter::per_minute(60).unwrap();
        let start = Instant::now();
        for i in 0..SWEEP_AT as u32 {
            limiter.check_at(IpAddr::from(i.to_be_bytes()), start).unwrap();
        }
        limiter.check_at(address(1), start + Duration::from_secs(60)).unwrap();
        assert_eq!(limiter.buckets.lock().unwrap().len(), 1);
    }
}
