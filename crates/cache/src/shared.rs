// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! Items shared by the native and wasm32 implementations of `SwrCache`.
//!
//! Nothing in this module touches `moka` or `tokio`, which is what makes it
//! shareable. The two implementations differ in the cache itself and nowhere
//! else, so configuration, the loader alias and the counters cannot drift
//! between targets.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;

/// Configuration for an `SwrCache`.
#[derive(Debug, Clone)]
pub struct SwrCacheConfig {
    /// Hard TTL — entries older than this are full misses.
    pub ttl: Duration,
    /// Soft TTL — entries older than this trigger background refresh on access.
    /// Must be `<= ttl`.
    pub soft_ttl: Duration,
    /// TTL applied to negative entries (`Ok(None)` from the loader).
    /// Must be `<= ttl`.
    pub negative_ttl: Duration,
    /// Maximum number of entries before LRU eviction kicks in. Must be `> 0`.
    pub max_entries: u64,
    /// Optional name used in logs and metrics. Defaults to "swr-cache".
    pub name: &'static str,
}

impl Default for SwrCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(60),
            soft_ttl: Duration::from_secs(30),
            negative_ttl: Duration::from_secs(5),
            max_entries: 10_000,
            name: "swr-cache",
        }
    }
}

/// Reasons a [`SwrCacheConfig`] may fail validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    ZeroTtl,
    ZeroMaxEntries,
    SoftTtlExceedsTtl,
    NegativeTtlExceedsTtl,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::ZeroTtl => "ttl must be > 0",
            Self::ZeroMaxEntries => "max_entries must be > 0",
            Self::SoftTtlExceedsTtl => "soft_ttl must be <= ttl",
            Self::NegativeTtlExceedsTtl => "negative_ttl must be <= ttl",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ConfigError {}

impl SwrCacheConfig {
    /// Validate config invariants. Operators that accept untrusted config
    /// (e.g. TOML) MUST call this at startup; bad values silently produce a
    /// thrash cache otherwise.
    ///
    /// # Errors
    /// Returns `ConfigError` describing the first invariant violation found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.ttl.is_zero() {
            return Err(ConfigError::ZeroTtl);
        }
        if self.max_entries == 0 {
            return Err(ConfigError::ZeroMaxEntries);
        }
        if self.soft_ttl > self.ttl {
            return Err(ConfigError::SoftTtlExceedsTtl);
        }
        if self.negative_ttl > self.ttl {
            return Err(ConfigError::NegativeTtlExceedsTtl);
        }
        Ok(())
    }
}

/// Type alias for a boxed loader future.
///
/// Loaders are stored as `Arc<dyn Fn(K) -> BoxFuture<...>>` so the cache can
/// invoke them from both the request path and spawned refresh tasks.
///
/// # Concurrency contract
///
/// Loaders may be invoked concurrently across **different keys**. Within a
/// single key, the cache deduplicates concurrent invocations on hard miss
/// via moka's single-flight semantics; only one loader future runs and all
/// racing callers share its outcome. Background refreshes are deduped via
/// the per-entry `refresh_in_flight` flag.
///
/// Loaders MUST be effect-free with respect to the cache's externally-
/// observable state. Capturing shared `Arc<dyn ...Store>` handles is fine;
/// capturing a `Mutex` or `mpsc::Sender` is a mistake.
pub type Loader<K, V, E> = Arc<dyn Fn(K) -> BoxFuture<'static, Result<Option<V>, E>> + Send + Sync>;

/// Atomic counters exposed for observability. Cheap to read; cheap to
/// `clone` (it returns an `Arc`-shared handle).
#[derive(Debug, Default)]
pub struct SwrMetrics {
    pub hits: AtomicU64,
    pub stale_hits: AtomicU64,
    pub misses: AtomicU64,
    pub negative_hits: AtomicU64,
    pub refresh_success: AtomicU64,
    pub refresh_failure: AtomicU64,
    pub refresh_skipped_inflight: AtomicU64,
    /// Refreshes whose result was discarded because the cache's epoch
    /// advanced (i.e. an explicit invalidation happened during the refresh).
    /// High counts mean the cache is doing wasted refresh work; low counts
    /// are the typical case.
    pub refresh_dropped_epoch: AtomicU64,
    pub invalidations: AtomicU64,
}

impl SwrMetrics {
    pub(crate) fn incr(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot all counters for export (e.g. to Prometheus).
    #[must_use]
    pub fn snapshot(&self) -> SwrMetricsSnapshot {
        SwrMetricsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            stale_hits: self.stale_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            negative_hits: self.negative_hits.load(Ordering::Relaxed),
            refresh_success: self.refresh_success.load(Ordering::Relaxed),
            refresh_failure: self.refresh_failure.load(Ordering::Relaxed),
            refresh_skipped_inflight: self.refresh_skipped_inflight.load(Ordering::Relaxed),
            refresh_dropped_epoch: self.refresh_dropped_epoch.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
        }
    }
}

/// Plain-old-data view of `SwrMetrics`. Useful for tests and metric exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SwrMetricsSnapshot {
    pub hits: u64,
    pub stale_hits: u64,
    pub misses: u64,
    pub negative_hits: u64,
    pub refresh_success: u64,
    pub refresh_failure: u64,
    pub refresh_skipped_inflight: u64,
    #[serde(default)]
    pub refresh_dropped_epoch: u64,
    pub invalidations: u64,
}
