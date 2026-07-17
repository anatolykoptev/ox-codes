use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use moka::sync::Cache;

/// A byte-weighted moka cache whose capacity (BYTES) and TTL are configurable
/// via per-call-site env vars, parameterized by a caller-supplied weigher.
///
/// This is the shared seam extracted from the two previously copy-pasted
/// caches: `ScopeCache` (`crates/core/src/scope_cache.rs`) and `DataflowCache`
/// (`crates/server/src/dataflow_cache.rs`). The two were structurally identical
/// (sim≈0.91) except for the value type, the env-var names, the defaults, and
/// the weigher closure — all of which are now constructor parameters here.
///
/// # Env-var semantics (apply to BYTES and TTL alike)
/// - explicit `0` is a valid, intentional value — TTL=0 = no expiry (the
///   `time_to_live` builder step is skipped entirely, so a hot unchanged entry
///   never expires), BYTES=0 = disable the cache (`max_capacity(0)`, every
///   entry evicted immediately → every request a miss — a real kill-switch).
///   It is NOT remapped to the default and does NOT warn (except the dedicated
///   TTL=0 staleness-backstop-disabled warning, see `ttl_zero_warning`).
/// - any other parseable u64 is used as-is.
/// - an UNPARSEABLE value falls back to the default and warns.
/// - an ABSENT value falls back to the default silently.
#[derive(Clone)]
pub struct WeightedEnvCache<K, V> {
    cache: Cache<K, V>,
    stats: Arc<CacheStats>,
}

struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    /// Number of times the `init` closure actually ran (i.e. real misses that
    /// did the expensive work). Exposed by `DataflowCache` as the third
    /// `stats()` element; `ScopeCache` ignores it.
    analyses: AtomicU64,
}

impl<K, V> WeightedEnvCache<K, V>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Build a cache with an explicit byte ceiling and TTL (in seconds).
    ///
    /// `ttl_secs == 0` means "no TTL" (an explicit escape hatch): the
    /// `time_to_live` builder step is skipped, so a hot unchanged entry never
    /// expires. Any other value bounds worst-case staleness to that many
    /// seconds.
    ///
    /// `ttl_env` is the env-var name used in the TTL=0 staleness-backstop
    /// warning; `cache_label` and `stale_noun` parameterize the warning prose
    /// (e.g. "scope cache"/"scope" for `ScopeCache`, "cache"/"result" for
    /// `DataflowCache`) so each cache's warning text is preserved verbatim.
    ///
    /// `weigher` sizes each entry by its retained byte weight; without it moka
    /// defaults to 1-per-entry and the cache is bounded by entry count rather
    /// than bytes.
    pub fn with_capacity_and_ttl(
        bytes: u64,
        ttl_secs: u64,
        ttl_env: &'static str,
        cache_label: &'static str,
        stale_noun: &'static str,
        weigher: impl Fn(&K, &V) -> u32 + Send + Sync + 'static,
    ) -> Self {
        if let Some(msg) = ttl_zero_warning(ttl_secs, ttl_env, cache_label, stale_noun) {
            tracing::warn!("{}", msg);
        }
        let mut builder = Cache::builder().max_capacity(bytes).weigher(weigher);
        if ttl_secs > 0 {
            builder = builder.time_to_live(Duration::from_secs(ttl_secs));
        }
        let cache = builder.build();

        Self {
            cache,
            stats: Arc::new(CacheStats {
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
                analyses: AtomicU64::new(0),
            }),
        }
    }

    /// Return `(hits, misses, analyses)` counters.
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.stats.hits.load(Ordering::Relaxed),
            self.stats.misses.load(Ordering::Relaxed),
            self.stats.analyses.load(Ordering::Relaxed),
        )
    }

    /// Return the number of entries in the cache.
    ///
    /// Runs pending internal maintenance first so the count is accurate.
    pub fn entry_count(&self) -> u64 {
        self.cache.run_pending_tasks();
        self.cache.entry_count()
    }

    /// Get or insert the cached value for `key`.
    /// `init` is called on a miss and must produce the value. Errors are NOT
    /// cached. Returns the value and a flag that is `true` on cache hit (the
    /// hit/miss observability signal consumed by the dataflow handler to set
    /// `DataflowResponse.is_hit`).
    pub fn get_or_insert<F>(&self, key: K, init: F) -> Result<(V, bool)>
    where
        F: FnOnce() -> Result<V>,
    {
        let was_miss = Arc::new(AtomicBool::new(false));
        let stats = Arc::clone(&self.stats);

        let value = self.cache.try_get_with(key, {
            let was_miss = Arc::clone(&was_miss);
            move || {
                was_miss.store(true, Ordering::Relaxed);
                stats.analyses.fetch_add(1, Ordering::Relaxed);
                init()
            }
        });

        match value {
            Ok(v) => {
                let is_hit = !was_miss.load(Ordering::Relaxed);
                if is_hit {
                    self.stats.hits.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.misses.fetch_add(1, Ordering::Relaxed);
                }
                Ok((v, is_hit))
            }
            Err(e) => {
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                Err(anyhow::anyhow!("{e}"))
            }
        }
    }

    /// Get or insert with content-hash verification (#48).
    ///
    /// Like `get_or_insert`, but on a key match the `verify` closure is called
    /// with the cached value. If `verify` returns `false` the entry is treated
    /// as STALE: it is invalidated and `init` re-runs to produce a fresh value.
    /// This closes the mtime+len fingerprint gap where a same-length in-place
    /// edit within the filesystem's mtime resolution leaves the key unchanged
    /// but the content different.
    ///
    /// The `verify` closure is responsible for any I/O it needs (e.g.
    /// re-reading the file and comparing a content hash). It is only called on
    /// the key-match path — a clear miss (different mtime/len) skips it
    /// entirely, so the hot-path cost is bounded to the cases where the cheap
    /// fingerprint already matched.
    pub fn get_or_insert_verified<F, Vf>(&self, key: K, init: F, verify: Vf) -> Result<(V, bool)>
    where
        F: FnOnce() -> Result<V>,
        Vf: FnOnce(&V) -> bool,
    {
        // Fast path: existing entry that passes content verification.
        if let Some(v) = self.cache.get(&key) {
            if verify(&v) {
                self.stats.hits.fetch_add(1, Ordering::Relaxed);
                return Ok((v, true));
            }
            // Stale (key matched but content differs): evict so the
            // try_get_with below re-runs init instead of returning the
            // stale value.
            self.cache.invalidate(&key);
        }
        // Miss or stale-then-evicted: run init and insert.
        let stats = Arc::clone(&self.stats);
        let value = self.cache.try_get_with(key, move || {
            stats.analyses.fetch_add(1, Ordering::Relaxed);
            init()
        });

        match value {
            Ok(v) => {
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                Ok((v, false))
            }
            Err(e) => {
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                Err(anyhow::anyhow!("{e}"))
            }
        }
    }
}

/// Read `env` and resolve it to a u64, falling back to `default` on
/// absent/unparseable values (see `resolve_env_u64` for the exact rules).
pub fn parse_env_u64(env: &str, default: u64) -> u64 {
    resolve_env_u64(std::env::var(env).ok(), env, default)
}

/// Pure resolution of a (possibly absent/unparseable) env value, separated from
/// the `std::env::var` read so it is unit-testable without touching the
/// process-global environment.
///
/// Env-var semantics (apply to BYTES and TTL alike):
/// - explicit `0` is a valid, intentional value — TTL=0 = no expiry,
///   BYTES=0 = disable the cache (`max_capacity(0)`). It is NOT remapped to
///   the default and does NOT warn.
/// - any other parseable u64 is used as-is.
/// - an UNPARSEABLE value falls back to the default and warns.
/// - an ABSENT value falls back to the default silently.
pub fn resolve_env_u64(raw: Option<String>, env: &str, default: u64) -> u64 {
    match raw.as_deref().and_then(|s| s.parse::<u64>().ok()) {
        Some(0) => 0,
        Some(v) => v,
        None => {
            if let Some(r) = raw {
                tracing::warn!("{}={:?} is unparseable; using default {}", env, r, default);
            }
            default
        }
    }
}

/// Pure helper: return a staleness-backstop-disabled warning message iff
/// `ttl_secs == 0` (the never-expire escape hatch), else `None`.
///
/// `TTL=0` inverts the near-universal ops convention (`Cache-Control: max-age=0`
/// = expire immediately) — here it means *never* expire. Combined with a
/// mtime+len fingerprint, a same-length in-place edit within mtime resolution
/// can serve a stale result until restart. This warning makes the footgun
/// loud without changing the intentional behavior.
///
/// `cache_label` and `stale_noun` parameterize the prose so each cache's
/// warning text is preserved verbatim across the refactor.
///
/// Separated from the `tracing::warn!` call so it is unit-testable without a
/// tracing subscriber.
fn ttl_zero_warning(
    ttl_secs: u64,
    env: &'static str,
    cache_label: &str,
    stale_noun: &str,
) -> Option<String> {
    if ttl_secs == 0 {
        Some(format!(
            "{env}=0: {cache_label} staleness backstop is DISABLED (TTL=0 means never-expire here, \
             the opposite of the max-age=0 convention). Combined with the mtime+len \
             fingerprint, a same-length in-place edit within mtime resolution can serve \
             a stale {stale_noun} until restart. Set a non-zero TTL to re-enable the backstop.",
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal concrete instantiation of `WeightedEnvCache` for exercising
    /// the shared byte-weighted-eviction / TTL=0 / BYTES=0 behavior without
    /// depending on either cache's value type. The weigher mirrors
    /// `ScopeCache`'s `source.len()` discipline.
    fn make_cache(bytes: u64, ttl_secs: u64) -> WeightedEnvCache<String, Arc<[u8]>> {
        WeightedEnvCache::with_capacity_and_ttl(
            bytes,
            ttl_secs,
            "TEST_TTL_ENV",
            "test cache",
            "value",
            |_k, v: &Arc<[u8]>| v.len().min(u32::MAX as usize) as u32,
        )
    }

    fn make_value(n: usize) -> Arc<[u8]> {
        Arc::from(vec![0u8; n].into_boxed_slice())
    }

    /// TTL=0 means "no expiry": a hot entry survives a re-get.
    /// Reverting to `time_to_live(Duration::from_secs(0))` makes moka expire the
    /// entry immediately, so the re-get becomes a miss and this test REDS.
    #[test]
    fn test_ttl_zero_means_no_expiry() {
        let cache = make_cache(64 * 1024 * 1024, 0);
        let key = "ttl-zero".to_string();
        let val = make_value(8);

        let _ = cache
            .get_or_insert(key.clone(), || Ok(val.clone()))
            .unwrap();
        let (_, is_hit) = cache.get_or_insert(key, || Ok(val.clone())).unwrap();
        assert!(is_hit, "ttl=0 means no expiry: re-get must be a hit");
    }

    /// BYTES=0 (max_capacity(0)) is the cache kill-switch: every entry is
    /// evicted immediately, so a repeat get is always a miss.
    /// Reverting to 0→default makes the entry survive, so the re-get becomes a
    /// hit and this test REDS.
    #[test]
    fn test_capacity_zero_disables_cache() {
        let cache = make_cache(0, 300);
        let key = "cap-zero".to_string();
        let val = make_value(8);

        let _ = cache
            .get_or_insert(key.clone(), || Ok(val.clone()))
            .unwrap();
        // Force moka maintenance so the over-capacity entry is actually evicted.
        let _ = cache.entry_count();
        let (_, is_hit) = cache.get_or_insert(key, || Ok(val.clone())).unwrap();
        assert!(
            !is_hit,
            "capacity=0 disables the cache: re-get must be a miss"
        );
    }

    /// Cache eviction must be driven by byte weight, not entry count. Two
    /// entries whose combined byte weight exceeds the ceiling must evict the
    /// older one — even though the entry count (2) is far below any reasonable
    /// entry-count cap. Reverting to a no-weigher cache (bounded by entry
    /// count) REDS this test: both entries survive.
    #[test]
    fn test_cache_evicts_by_byte_weight() {
        // Ceiling large enough for one large entry but NOT two.
        let cache = make_cache(1500, 0);

        let key1 = "byte-1".to_string();
        let key2 = "byte-2".to_string();
        let val1 = make_value(1000);
        let val2 = make_value(1000);

        let _ = cache
            .get_or_insert(key1.clone(), || Ok(val1.clone()))
            .unwrap();
        // Force moka maintenance so the entry is fully registered.
        let _ = cache.entry_count();

        let _ = cache
            .get_or_insert(key2.clone(), || Ok(val2.clone()))
            .unwrap();
        // Force maintenance so the over-capacity entry is actually evicted.
        let _ = cache.entry_count();

        // key2 should still be cached (it was inserted second).
        let (_, is_hit2) = cache.get_or_insert(key2, || Ok(val2.clone())).unwrap();
        assert!(is_hit2, "key2 should still be cached (newer entry)");

        // key1 should have been evicted by byte weight.
        let (_, is_hit1) = cache.get_or_insert(key1, || Ok(val1.clone())).unwrap();
        assert!(
            !is_hit1,
            "key1 should have been evicted by byte weight, not entry count"
        );
    }

    /// Env parser (u64, BYTES/TTL): an explicit "0" is a valid intentional
    /// value (the TTL=0 no-expiry / BYTES=0 kill-switch escape hatch), NOT
    /// remapped to the default. Reverting to the old `0 → default` mapping
    /// REDS this test.
    #[test]
    fn test_resolve_env_u64_zero_is_valid() {
        assert_eq!(
            resolve_env_u64(Some("0".to_string()), "TEST", 300),
            0,
            "explicit 0 must be preserved, not remapped to the default"
        );
    }

    /// Env parser (u64, BYTES/TTL): unparseable → default.
    #[test]
    fn test_resolve_env_u64_unparseable_falls_back() {
        assert_eq!(
            resolve_env_u64(Some("garbage".to_string()), "TEST", 300),
            300,
            "unparseable value must fall back to the default"
        );
    }

    /// Env parser (u64, BYTES/TTL): absent → default.
    #[test]
    fn test_resolve_env_u64_absent_falls_back() {
        assert_eq!(
            resolve_env_u64(None, "TEST", 300),
            300,
            "absent value must fall back to the default"
        );
    }

    /// TTL=0 disables the staleness backstop (never-expire). The pure helper
    /// must return a warning message iff ttl_secs == 0, so the constructor can
    /// emit a loud `tracing::warn!` without needing a tracing subscriber in
    /// tests. Reverting the helper (always returning None) REDS this test.
    #[test]
    fn test_ttl_zero_warning_returns_some_for_zero() {
        assert!(
            ttl_zero_warning(0, "TEST_TTL_ENV", "test cache", "value").is_some(),
            "ttl=0 must produce a staleness-backstop-disabled warning"
        );
    }

    /// Any non-zero TTL keeps the staleness backstop active — no warning.
    /// Reverting the helper (always returning Some) REDS this test.
    #[test]
    fn test_ttl_zero_warning_returns_none_for_nonzero() {
        assert!(
            ttl_zero_warning(300, "TEST_TTL_ENV", "test cache", "value").is_none(),
            "default ttl must not warn"
        );
        assert!(
            ttl_zero_warning(1, "TEST_TTL_ENV", "test cache", "value").is_none(),
            "ttl=1 must not warn"
        );
    }
}
