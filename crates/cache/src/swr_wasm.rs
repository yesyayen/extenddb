// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! wasm32 build of the stale-while-revalidate cache: pass-through only.
//!
//! The native implementation is built on `moka` for storage and
//! `tokio::spawn` for background refresh. Neither targets
//! wasm32-unknown-unknown, so on that target this module supplies the same
//! public API with no storage behind it: every `get` calls the loader and
//! every `invalidate*` does nothing.
//!
//! This is the behaviour the native crate already has when constructed via
//! `SwrCache::pass_through`, which the server uses for
//! `auth.cache.enabled = false`. Matching it exactly, rather than inventing a
//! third behaviour, means the native pass-through tests describe this target
//! too. In particular `invalidate*` leaves the epoch and the invalidation
//! counter alone here, because the native pass-through path returns before
//! touching either.

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use crate::shared::{ConfigError, Loader, SwrCacheConfig, SwrMetrics};

/// Stand-in for `moka::PredicateError`, which the native build re-exports as
/// part of this crate's API. moka is not compiled on wasm32, so callers get
/// this type instead and need no moka dependency of their own. The
/// invalidate-by-predicate methods are no-ops here, so it is never returned.
///
/// The variant exists for one reason: on native it is public API, so a `match`
/// over this error compiles there, and it must compile here too or the two
/// targets accept different code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateError {
    /// Never produced on this target. Named to match moka's variant.
    InvalidationClosuresDisabled,
}

impl std::fmt::Display for PredicateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately not moka's wording, which tells the reader to call
        // support_invalidation_closures on a cache builder that does not exist
        // on this target.
        match self {
            Self::InvalidationClosuresDisabled => {
                f.write_str("invalidation by predicate is a no-op on wasm32")
            }
        }
    }
}

impl std::error::Error for PredicateError {}

/// Stale-while-revalidate cache, wasm32 build.
///
/// Nothing is cached and nothing is spawned. Cloning is cheap: clones share
/// the loader, configuration and metrics.
pub struct SwrCache<K, V, E>
where
    K: Hash + Eq + Send + Sync + Clone + 'static,
    V: Clone + Send + Sync + 'static,
    E: Clone + Send + Sync + 'static,
{
    loader: Loader<K, V, E>,
    config: Arc<SwrCacheConfig>,
    metrics: Arc<SwrMetrics>,
}

impl<K, V, E> Clone for SwrCache<K, V, E>
where
    K: Hash + Eq + Send + Sync + Clone + 'static,
    V: Clone + Send + Sync + 'static,
    E: Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            loader: self.loader.clone(),
            config: self.config.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

impl<K, V, E> SwrCache<K, V, E>
where
    K: Hash + Eq + Send + Sync + Clone + Debug + 'static,
    V: Clone + Send + Sync + 'static,
    E: Clone + Send + Sync + Debug + 'static,
{
    /// Construct a cache. On wasm32 this is pass-through whatever the TTLs
    /// say, so it is `try_pass_through` under another name. `config` is still
    /// validated, so a configuration rejected on native is rejected here.
    ///
    /// # Errors
    /// Returns `ConfigError` if `config` fails [`SwrCacheConfig::validate`].
    pub fn try_new(loader: Loader<K, V, E>, config: SwrCacheConfig) -> Result<Self, ConfigError> {
        Self::try_pass_through(loader, config)
    }

    /// Construct a pass-through cache: every `get` invokes the loader, every
    /// `invalidate*` is a no-op.
    ///
    /// # Errors
    /// Returns `ConfigError` if `config` fails [`SwrCacheConfig::validate`].
    pub fn try_pass_through(
        loader: Loader<K, V, E>,
        config: SwrCacheConfig,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self {
            loader,
            config: Arc::new(config),
            metrics: Arc::new(SwrMetrics::default()),
        })
    }

    /// Construct a pass-through cache, panicking on invalid configuration.
    ///
    /// # Panics
    /// Panics if `config` fails [`SwrCacheConfig::validate`].
    #[must_use]
    pub fn pass_through(loader: Loader<K, V, E>, config: SwrCacheConfig) -> Self {
        Self::try_pass_through(loader, config).expect("invalid SwrCacheConfig")
    }

    /// Construct a cache, panicking on invalid configuration.
    ///
    /// # Panics
    /// Panics if `config` fails [`SwrCacheConfig::validate`].
    #[must_use]
    pub fn new(loader: Loader<K, V, E>, config: SwrCacheConfig) -> Self {
        Self::try_new(loader, config).expect("invalid SwrCacheConfig")
    }

    /// Returns a shared handle to the metrics. Cheap to clone.
    ///
    /// One counter carries information on this target. `misses` equals the
    /// number of `get` calls, because every `get` reaches the loader. The rest
    /// are structurally zero rather than merely unpopulated, and for one
    /// reason: they all need a stored entry, whether to serve, to mark, to
    /// replace or to discard, and nothing is stored. `invalidations` is the
    /// exception and it stays at zero for its own reason, that `invalidate*`
    /// returns without counting, which is what the native pass-through path
    /// does too.
    ///
    /// The accessor is kept rather than gated because
    /// `extenddb_auth::CachedCredentialStore::metrics` forwards it
    /// unconditionally and that crate is in the wasm graph. Nothing in a wasm
    /// build reads the snapshot: every caller of `SwrMetrics::snapshot` outside
    /// this crate's own tests is in `crates/server`, which is not in that
    /// graph, and the tests do not compile on this target.
    #[must_use]
    pub fn metrics(&self) -> Arc<SwrMetrics> {
        self.metrics.clone()
    }

    /// Returns the configured cache name (for logging / metric labelling).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.config.name
    }

    // Several methods below keep the native signature over a body that is
    // empty or constant by design. Where that leaves an `async fn` with no
    // await, the lint is allowed rather than the signature changed: the point
    // of this module is that the two targets present the same API.

    /// Always 0: there is no underlying storage on this target.
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        0
    }

    /// Always `true` on this target.
    #[must_use]
    pub fn is_pass_through(&self) -> bool {
        true
    }

    /// Look up a key. Always calls the loader, and counts a miss so the
    /// bypass is visible in the metrics, exactly as native pass-through does.
    ///
    /// # Errors
    /// Propagates any error from the loader.
    pub async fn get(&self, key: K) -> Result<Option<V>, E> {
        SwrMetrics::incr(&self.metrics.misses);
        (self.loader)(key).await
    }

    /// No-op: nothing is cached.
    #[allow(clippy::unused_async)]
    pub async fn invalidate(&self, _key: &K) {}

    /// No-op: nothing is cached.
    ///
    /// # Errors
    /// Never fails on this target.
    pub fn invalidate_if<F>(&self, _key_predicate: F) -> Result<(), PredicateError>
    where
        F: Fn(&K) -> bool + Send + Sync + 'static,
    {
        Ok(())
    }

    /// No-op: nothing is cached.
    ///
    /// # Errors
    /// Never fails on this target.
    pub fn invalidate_if_value<F>(&self, _value_predicate: F) -> Result<(), PredicateError>
    where
        F: Fn(Option<&V>) -> bool + Send + Sync + 'static,
    {
        Ok(())
    }

    /// No-op: nothing is cached.
    pub fn invalidate_all(&self) {}

    /// Returns the epoch counter, which is 0 on every call.
    ///
    /// Native advances it on invalidation so a racing load can discard its
    /// result. Nothing here can race: no load is shared and no result is
    /// stored. Returned as a constant rather than read from a field nothing
    /// writes, which matches `entry_count` above and keeps an always-zero
    /// atomic out of a wasm bundle.
    #[doc(hidden)]
    #[must_use]
    pub fn epoch(&self) -> u64 {
        0
    }

    /// No-op, which is what native does in pass-through mode too: its body is
    /// guarded on the moka handle and pass-through carries none. Nothing is
    /// stored on this target, so there is no housekeeping to drain.
    ///
    /// Available only with the `test-util` feature, or to other tests inside
    /// this crate, matching the native implementation.
    #[cfg(any(test, feature = "test-util"))]
    #[allow(clippy::unused_async)]
    pub async fn run_pending_tasks(&self) {}
}
