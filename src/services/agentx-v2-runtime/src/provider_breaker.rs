//! Provider circuit breaker and fairness limiter (plan7 P7-D3).
//!
//! The breaker opens per (tenant, provider host) after consecutive send-level
//! failures and fails fast with a deterministic `PROVIDER_CIRCUIT_OPEN` while
//! cooling down (half-open probe after the cooldown). The fairness limiter
//! caps in-flight provider calls per (tenant, call kind, provider host) with
//! an owned semaphore permit that releases on every early return.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const FAILURE_THRESHOLD: u32 = 5;
const COOLDOWN: Duration = Duration::from_millis(30_000);
const DEFAULT_MAX_INFLIGHT: usize = 32;

pub const PROVIDER_CIRCUIT_OPEN: &str = "PROVIDER_CIRCUIT_OPEN";
pub const PROVIDER_BUSY: &str = "PROVIDER_BUSY";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BreakerState {
    Closed,
    Open { until: Instant },
}

struct BreakerEntry {
    state: BreakerState,
    consecutive_failures: u32,
}

#[derive(Default)]
pub struct ProviderBreaker {
    entries: Mutex<HashMap<String, BreakerEntry>>,
}

impl ProviderBreaker {
    pub fn shared() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    /// Fails fast while the breaker is open; a probe after the cooldown is
    /// allowed through (half-open) so recovery does not wait for an operator.
    pub fn allow(&self, key: &str, now: Instant) -> bool {
        let mut entries = self.entries.lock().expect("provider breaker lock");
        match entries.get_mut(key) {
            None => true,
            Some(entry) => match entry.state {
                BreakerState::Closed => true,
                BreakerState::Open { until } => {
                    if now >= until {
                        // Half-open: let this probe through; a failure
                        // re-opens immediately, a success closes the breaker.
                        entry.state = BreakerState::Closed;
                        entry.consecutive_failures = FAILURE_THRESHOLD - 1;
                        true
                    } else {
                        false
                    }
                }
            },
        }
    }

    pub fn record_success(&self, key: &str) {
        let mut entries = self.entries.lock().expect("provider breaker lock");
        if let Some(entry) = entries.get_mut(key) {
            entry.state = BreakerState::Closed;
            entry.consecutive_failures = 0;
        }
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut entries = self.entries.lock().expect("provider breaker lock");
        let entry = entries.entry(key.to_owned()).or_insert(BreakerEntry {
            state: BreakerState::Closed,
            consecutive_failures: 0,
        });
        if entry.state == BreakerState::Closed {
            entry.consecutive_failures += 1;
            if entry.consecutive_failures >= FAILURE_THRESHOLD {
                entry.state = BreakerState::Open {
                    until: now + COOLDOWN,
                };
            }
        } else if let BreakerState::Open { until } = entry.state {
            if now >= until {
                // Failed probe: re-open for another cooldown window.
                entry.state = BreakerState::Open {
                    until: now + COOLDOWN,
                };
            }
        }
    }

    pub fn open_count(&self) -> usize {
        self.entries
            .lock()
            .expect("provider breaker lock")
            .values()
            .filter(|entry| matches!(entry.state, BreakerState::Open { .. }))
            .count()
    }

    pub fn failure_threshold() -> u32 {
        FAILURE_THRESHOLD
    }

    pub fn cooldown() -> Duration {
        COOLDOWN
    }
}

/// Per-(tenant, kind, host) in-flight cap. Permits are owned and release on
/// drop, so every early return in the caller's send path frees the slot.
#[derive(Default)]
pub struct FairnessLimiter {
    semaphores: Mutex<HashMap<String, std::sync::Arc<tokio::sync::Semaphore>>>,
    per_key: usize,
    peak_inflight: std::sync::atomic::AtomicUsize,
}

impl FairnessLimiter {
    pub fn shared() -> std::sync::Arc<Self> {
        let per_key = std::env::var("AGENTX_PROVIDER_MAX_INFLIGHT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_MAX_INFLIGHT)
            .clamp(1, 4096);
        std::sync::Arc::new(Self {
            per_key,
            ..Self::default()
        })
    }

    pub fn try_acquire(&self, key: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let semaphore = {
            let mut semaphores = self.semaphores.lock().expect("fairness limiter lock");
            semaphores
                .entry(key.to_owned())
                .or_insert_with(|| std::sync::Arc::new(tokio::sync::Semaphore::new(self.per_key)))
                .clone()
        };
        let permit = semaphore.clone().try_acquire_owned().ok()?;
        self.peak_inflight.fetch_max(
            self.per_key.saturating_sub(semaphore.available_permits()),
            std::sync::atomic::Ordering::Relaxed,
        );
        Some(permit)
    }
    pub fn peak_utilization(&self) -> f64 {
        self.peak_inflight
            .load(std::sync::atomic::Ordering::Relaxed) as f64
            / self.per_key.max(1) as f64
    }
}

/// Breaker/limiter key: tenant + call kind + endpoint host.
pub fn provider_key(tenant_id: uuid::Uuid, kind: &str, endpoint: &str) -> String {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| endpoint.to_owned());
    format!("{tenant_id}:{kind}:{host}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_consecutive_failures_and_recovers() {
        let breaker = ProviderBreaker::default();
        let now = Instant::now();
        for _ in 0..FAILURE_THRESHOLD - 1 {
            breaker.record_failure("k", now);
            assert!(breaker.allow("k", now));
        }
        breaker.record_failure("k", now);
        assert!(!breaker.allow("k", now), "open after threshold");
        // Half-open probe passes after the cooldown...
        assert!(breaker.allow("k", now + COOLDOWN));
        // ...and a success closes it again.
        breaker.record_success("k");
        assert!(breaker.allow("k", now + COOLDOWN));
    }

    #[test]
    fn failed_half_open_probe_reopens() {
        let breaker = ProviderBreaker::default();
        let now = Instant::now();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.record_failure("k", now);
        }
        assert!(breaker.allow("k", now + COOLDOWN), "probe passes");
        breaker.record_failure("k", now + COOLDOWN);
        assert!(
            !breaker.allow("k", now + COOLDOWN + Duration::from_millis(1)),
            "failed probe re-opens"
        );
    }

    #[tokio::test]
    async fn fairness_limiter_caps_inflight_and_releases() {
        let limiter = FairnessLimiter {
            per_key: 2,
            ..FairnessLimiter::default()
        };
        let first = limiter.try_acquire("k").expect("first permit");
        let second = limiter.try_acquire("k").expect("second permit");
        assert!(limiter.try_acquire("k").is_none(), "cap reached");
        drop(second);
        assert!(
            limiter.try_acquire("k").is_some(),
            "permit released on drop"
        );
        drop(first);
    }

    #[test]
    fn provider_key_uses_endpoint_host() {
        let key = provider_key(
            uuid::Uuid::nil(),
            "model",
            "https://api.example.com/v1/chat/completions",
        );
        assert!(key.ends_with(":model:api.example.com"), "{key}");
    }
}
