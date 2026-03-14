use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;

use super::error::ApiError;
use super::AppState;

const MAX_ENTRIES: usize = 10_000;
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);
const PRUNE_COOLDOWN_SECS: u64 = 60;

pub struct RateLimiter {
    entries: DashMap<String, Entry>,
    per_minute: u32,
    per_hour: u32,
    last_prune: AtomicU64,
}

struct Entry {
    minute_count: u32,
    minute_start: Instant,
    hour_count: u32,
    hour_start: Instant,
}

struct CheckResult {
    allowed: bool,
    limit: u32,
    remaining: u32,
    reset_secs: u64,
    retry_after: Option<u64>,
}

impl RateLimiter {
    pub fn new(per_minute: u32, per_hour: u32) -> Self {
        Self {
            entries: DashMap::new(),
            per_minute,
            per_hour,
            last_prune: AtomicU64::new(0),
        }
    }

    fn check(&self, key: &str) -> CheckResult {
        let now = Instant::now();

        if self.entries.len() >= MAX_ENTRIES {
            let now_secs = unix_now();
            let last = self.last_prune.load(Ordering::Relaxed);
            if now_secs.saturating_sub(last) >= PRUNE_COOLDOWN_SECS
                && self
                    .last_prune
                    .compare_exchange(last, now_secs, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                self.entries.retain(|_, e| {
                    now.duration_since(e.minute_start) < MINUTE
                        || now.duration_since(e.hour_start) < HOUR
                });
            }
        }

        // Avoid allocating a String on every request — only allocate on cache miss
        let mut entry = if let Some(existing) = self.entries.get_mut(key) {
            existing
        } else {
            self.entries
                .entry(key.to_string())
                .or_insert_with(|| Entry {
                    minute_count: 0,
                    minute_start: now,
                    hour_count: 0,
                    hour_start: now,
                })
        };

        if now.duration_since(entry.minute_start) >= MINUTE {
            entry.minute_count = 0;
            entry.minute_start = now;
        }
        if now.duration_since(entry.hour_start) >= HOUR {
            entry.hour_count = 0;
            entry.hour_start = now;
        }

        entry.minute_count += 1;
        entry.hour_count += 1;

        let unix_now = unix_now();

        if entry.minute_count > self.per_minute {
            let reset_in = window_remaining(entry.minute_start, MINUTE, now);
            return CheckResult {
                allowed: false,
                limit: self.per_minute,
                remaining: 0,
                reset_secs: unix_now + reset_in,
                retry_after: Some(reset_in),
            };
        }

        if entry.hour_count > self.per_hour {
            let reset_in = window_remaining(entry.hour_start, HOUR, now);
            return CheckResult {
                allowed: false,
                limit: self.per_hour,
                remaining: 0,
                reset_secs: unix_now + reset_in,
                retry_after: Some(reset_in),
            };
        }

        let minute_left = self.per_minute - entry.minute_count;
        let hour_left = self.per_hour - entry.hour_count;

        let (limit, remaining, secs_left) = if minute_left <= hour_left {
            (
                self.per_minute,
                minute_left,
                window_remaining(entry.minute_start, MINUTE, now),
            )
        } else {
            (
                self.per_hour,
                hour_left,
                window_remaining(entry.hour_start, HOUR, now),
            )
        };

        CheckResult {
            allowed: true,
            limit,
            remaining,
            reset_secs: unix_now + secs_left,
            retry_after: None,
        }
    }
}

fn window_remaining(start: Instant, duration: Duration, now: Instant) -> u64 {
    duration
        .checked_sub(now.duration_since(start))
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .max(1)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub async fn middleware(
    State(state): State<AppState>,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let key_hash = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
        .map(super::auth::hash_key);

    let Some(key_hash) = key_hash else {
        return next.run(request).await;
    };

    let result = state.rate_limiter.check(&key_hash);

    if !result.allowed {
        let mut resp = ApiError::RateLimited.into_response();
        set_rate_headers(resp.headers_mut(), &result);
        resp.headers_mut()
            .insert("Retry-After", header_val(result.retry_after.unwrap_or(1)));
        return resp;
    }

    let mut resp = next.run(request).await;
    set_rate_headers(resp.headers_mut(), &result);
    resp
}

fn header_val(n: u64) -> HeaderValue {
    HeaderValue::from_str(&n.to_string()).unwrap()
}

fn set_rate_headers(headers: &mut axum::http::HeaderMap, result: &CheckResult) {
    headers.insert("X-RateLimit-Limit", header_val(result.limit as u64));
    headers.insert("X-RateLimit-Remaining", header_val(result.remaining as u64));
    headers.insert("X-RateLimit-Reset", header_val(result.reset_secs));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_within_limit_passes() {
        let limiter = RateLimiter::new(60, 1000);
        let r = limiter.check("key_a");
        assert!(r.allowed);
        assert_eq!(r.limit, 60);
        assert_eq!(r.remaining, 59);
        assert!(r.retry_after.is_none());
    }

    #[test]
    fn test_rate_limit_over_minute_limit_blocked() {
        let limiter = RateLimiter::new(3, 1000);
        for _ in 0..3 {
            assert!(limiter.check("key_a").allowed);
        }
        let r = limiter.check("key_a");
        assert!(!r.allowed);
        assert_eq!(r.remaining, 0);
        assert_eq!(r.limit, 3);
        assert!(r.retry_after.unwrap() >= 1);
    }

    #[test]
    fn test_rate_limit_over_hour_limit_blocked() {
        let limiter = RateLimiter::new(1000, 3);
        for _ in 0..3 {
            assert!(limiter.check("key_a").allowed);
        }
        let r = limiter.check("key_a");
        assert!(!r.allowed);
        assert_eq!(r.remaining, 0);
        assert!(r.retry_after.unwrap() >= 1);
    }

    #[test]
    fn test_rate_limit_headers_correct_values() {
        let limiter = RateLimiter::new(10, 100);
        let r1 = limiter.check("key_a");
        assert_eq!(r1.limit, 10);
        assert_eq!(r1.remaining, 9);
        assert!(r1.reset_secs > 0);

        let r2 = limiter.check("key_a");
        assert_eq!(r2.remaining, 8);
    }

    #[test]
    fn test_rate_limit_minute_window_resets() {
        let limiter = RateLimiter::new(2, 1000);
        limiter.check("key_a");
        limiter.check("key_a");
        assert!(!limiter.check("key_a").allowed);

        let mut entry = limiter.entries.get_mut("key_a").unwrap();
        entry.minute_start = Instant::now() - Duration::from_secs(61);
        drop(entry);

        let r = limiter.check("key_a");
        assert!(r.allowed);
        assert_eq!(r.remaining, 1);
    }

    #[test]
    fn test_rate_limit_hour_window_resets() {
        let limiter = RateLimiter::new(1000, 2);
        limiter.check("key_a");
        limiter.check("key_a");
        assert!(!limiter.check("key_a").allowed);

        let mut entry = limiter.entries.get_mut("key_a").unwrap();
        entry.hour_start = Instant::now() - Duration::from_secs(3601);
        drop(entry);

        let r = limiter.check("key_a");
        assert!(r.allowed);
    }

    #[test]
    fn test_rate_limit_key_isolation() {
        let limiter = RateLimiter::new(2, 1000);
        limiter.check("key_a");
        limiter.check("key_a");
        assert!(!limiter.check("key_a").allowed);

        let r = limiter.check("key_b");
        assert!(r.allowed);
        assert_eq!(r.remaining, 1);
    }

    #[test]
    fn test_rate_limit_remaining_decrements() {
        let limiter = RateLimiter::new(5, 1000);
        for expected in (0..5).rev() {
            let r = limiter.check("key_a");
            assert_eq!(r.remaining, expected);
        }
    }

    #[test]
    fn test_rate_limit_prune_removes_expired() {
        let limiter = RateLimiter::new(100, 100);
        for i in 0..5 {
            limiter.check(&format!("key_{i}"));
        }
        assert_eq!(limiter.entries.len(), 5);

        for mut entry in limiter.entries.iter_mut() {
            entry.minute_start = Instant::now() - Duration::from_secs(61);
            entry.hour_start = Instant::now() - Duration::from_secs(3601);
        }

        let now = Instant::now();
        limiter.entries.retain(|_, e| {
            now.duration_since(e.minute_start) < MINUTE || now.duration_since(e.hour_start) < HOUR
        });
        assert_eq!(limiter.entries.len(), 0);
    }
}
