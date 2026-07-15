use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use moka::sync::Cache;
use ox_dataflow::DataflowResponse;

/// Default cap on the number of cached repo results.
/// A single `DataflowResponse` is one-per-repo and can be larger/variable than a
/// parsed-scope entry, so this cache is bounded by entry count rather than bytes.
const DEFAULT_CAPACITY: usize = 64;

/// Default TTL for dataflow result entries.
///
/// The aggregate fingerprint is mtime+len per file, which can serve stale results
/// for same-length in-place edits within the filesystem's mtime resolution. A
/// moka TTL bounds the worst-case staleness to this many seconds without paying
/// for a content hash of every file.
const DEFAULT_TTL_SECS: u64 = 300;

/// Environment variable override for the dataflow result cache entry limit.
///
/// `0` = disable the cache (`max_capacity(0)`, every entry evicted immediately
/// → every request a miss — a real kill-switch). Absent/unparseable → default.
pub const CACHE_ENTRIES_ENV: &str = "OX_CODES_DATAFLOW_CACHE_ENTRIES";

/// Environment variable override for the dataflow result cache entry TTL.
///
/// `0` = no TTL (do not set `time_to_live` — a hot unchanged entry never
/// expires). Absent/unparseable → default (300s).
pub const CACHE_TTL_ENV: &str = "OX_CODES_DATAFLOW_CACHE_TTL_SECS";

/// Cache key for the dataflow result cache.
///
/// The result depends on the analyzed file set (captured by `aggregate_fingerprint`
/// and capped by `max_files` folded into the fingerprint) and on `max_results`.
/// `file_glob` and `exclude_glob` are stored explicitly for stable keying even when
/// two different glob strings match the same file set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DataflowCacheKey {
    pub(crate) canonical_root: PathBuf,
    pub(crate) language: String,
    pub(crate) file_glob: Option<String>,
    pub(crate) exclude_glob: Option<String>,
    pub(crate) max_results: usize,
    pub(crate) aggregate_fingerprint: u64,
}

struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    analyses: AtomicU64,
}

/// Cross-request cache for whole-repo dataflow analysis results.
///
/// Key: repo root + language + include/exclude globs + max_results + an aggregate
/// fingerprint of the analyzed file set (mtime_nanos + file_len per file).
/// Value: `Arc<DataflowResponse>`.
///
/// Entries carry a moka TTL (default 300s, override via
/// `OX_CODES_DATAFLOW_CACHE_TTL_SECS`) that acts as a staleness backstop for the
/// mtime+len fingerprint without paying for a content hash of every file.
#[derive(Clone)]
pub struct DataflowCache {
    cache: Cache<DataflowCacheKey, Arc<DataflowResponse>>,
    stats: Arc<CacheStats>,
}

impl DataflowCache {
    pub fn new() -> Self {
        let capacity = parse_env_usize(CACHE_ENTRIES_ENV, DEFAULT_CAPACITY);
        let ttl_secs = parse_env_u64(CACHE_TTL_ENV, DEFAULT_TTL_SECS);
        Self::with_capacity_and_ttl(capacity, ttl_secs)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_ttl(capacity, DEFAULT_TTL_SECS)
    }

    fn with_capacity_and_ttl(capacity: usize, ttl_secs: u64) -> Self {
        // ttl_secs=0 means "no TTL" (an explicit escape hatch): do not set
        // `time_to_live` at all, so a hot unchanged entry never expires.
        // Any other value bounds worst-case staleness to that many seconds.
        let mut builder = Cache::builder().max_capacity(capacity as u64);
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

    /// Return (hits, misses, analysis-invocation) counters.
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

    /// Get or insert the cached result for `key`.
    /// `init` is called on a miss and must run the full analysis.
    /// Errors are NOT cached.
    /// Returns the value and a flag that is `true` on cache hit.
    pub fn get_or_insert<F>(
        &self,
        key: DataflowCacheKey,
        init: F,
    ) -> Result<(Arc<DataflowResponse>, bool)>
    where
        F: FnOnce() -> Result<Arc<DataflowResponse>>,
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
}

fn parse_env_u64(env: &str, default: u64) -> u64 {
    resolve_env_u64(std::env::var(env).ok(), env, default)
}

/// Pure resolution of a (possibly absent/unparseable) env value, separated from
/// the `std::env::var` read so it is unit-testable without touching the
/// process-global environment.
///
/// Env-var semantics (apply to TTL and ENTRIES alike):
/// - explicit `0` is a valid, intentional value — TTL=0 = no expiry,
///   ENTRIES=0 = disable the cache (`max_capacity(0)`). It is NOT remapped to
///   the default and does NOT warn.
/// - any other parseable u64 is used as-is.
/// - an UNPARSEABLE value falls back to the default and warns.
/// - an ABSENT value falls back to the default silently.
fn resolve_env_u64(raw: Option<String>, env: &str, default: u64) -> u64 {
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

/// Like `parse_env_u64` but for usize entry counts.
///
/// An explicit `0` is a valid, intentional value (ENTRIES=0 = disable the
/// cache via `max_capacity(0)` — the kill-switch). Do NOT remap it to the
/// default and do not warn — only an UNPARSEABLE or absent value falls back to
/// the default (with a warn on an unparseable value).
fn parse_env_usize(env: &str, default: usize) -> usize {
    resolve_env_usize(std::env::var(env).ok(), env, default)
}

/// Pure resolution of a usize env value (see `resolve_env_u64` for semantics).
fn resolve_env_usize(raw: Option<String>, env: &str, default: usize) -> usize {
    match raw.as_deref().and_then(|s| s.parse::<usize>().ok()) {
        Some(0) => 0,
        Some(c) => c,
        None => {
            if let Some(r) = raw {
                tracing::warn!("{}={:?} is unparseable; using default {}", env, r, default);
            }
            default
        }
    }
}

impl Default for DataflowCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(tag: &str) -> DataflowCacheKey {
        DataflowCacheKey {
            canonical_root: PathBuf::from(format!("/nonexistent/{tag}")),
            language: "go".to_string(),
            file_glob: None,
            exclude_glob: None,
            max_results: 100,
            aggregate_fingerprint: 1,
        }
    }

    fn make_value() -> Arc<DataflowResponse> {
        Arc::new(DataflowResponse {
            findings: Vec::new(),
            total_findings: 0,
            files_analyzed: 0,
            truncated: false,
            files_truncated: false,
            duration_ms: 0,
        })
    }

    /// TTL=0 means "no expiry": a hot entry survives a re-get.
    /// Reverting to `time_to_live(Duration::from_secs(0))` makes moka expire the
    /// entry immediately, so the re-get becomes a miss and this test REDS.
    #[test]
    fn test_ttl_zero_means_no_expiry() {
        let cache = DataflowCache::with_capacity_and_ttl(64, 0);
        let key = make_key("ttl-zero");
        let val = make_value();

        let _ = cache
            .get_or_insert(key.clone(), || Ok(val.clone()))
            .unwrap();
        let (_, is_hit) = cache.get_or_insert(key, || Ok(val.clone())).unwrap();
        assert!(is_hit, "ttl=0 means no expiry: re-get must be a hit");
    }

    /// ENTRIES=0 (max_capacity(0)) is the cache kill-switch: every entry is
    /// evicted immediately, so a repeat get is always a miss.
    /// Reverting to 0→default makes the entry survive, so the re-get becomes a
    /// hit and this test REDS.
    #[test]
    fn test_capacity_zero_disables_cache() {
        let cache = DataflowCache::with_capacity_and_ttl(0, DEFAULT_TTL_SECS);
        let key = make_key("cap-zero");
        let val = make_value();

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

    /// Env parser (u64, TTL): an explicit "0" is a valid intentional value
    /// (the TTL=0 no-expiry escape hatch), NOT remapped to the default.
    /// Reverting to the old `0 → default` mapping REDS this test.
    #[test]
    fn test_resolve_env_u64_zero_is_valid() {
        assert_eq!(
            resolve_env_u64(Some("0".to_string()), "TEST", 300),
            0,
            "explicit 0 must be preserved, not remapped to the default"
        );
    }

    /// Env parser (u64, TTL): unparseable → default.
    #[test]
    fn test_resolve_env_u64_unparseable_falls_back() {
        assert_eq!(
            resolve_env_u64(Some("garbage".to_string()), "TEST", 300),
            300,
            "unparseable value must fall back to the default"
        );
    }

    /// Env parser (u64, TTL): absent → default.
    #[test]
    fn test_resolve_env_u64_absent_falls_back() {
        assert_eq!(
            resolve_env_u64(None, "TEST", 300),
            300,
            "absent value must fall back to the default"
        );
    }

    /// Env parser (usize, ENTRIES): an explicit "0" is the disable kill-switch,
    /// NOT remapped to the default. Reverting to `0 → default` REDS this test.
    #[test]
    fn test_resolve_env_usize_zero_is_valid() {
        assert_eq!(
            resolve_env_usize(Some("0".to_string()), "TEST", 64),
            0,
            "explicit 0 must be preserved, not remapped to the default"
        );
    }

    /// Env parser (usize, ENTRIES): unparseable → default.
    #[test]
    fn test_resolve_env_usize_unparseable_falls_back() {
        assert_eq!(
            resolve_env_usize(Some("garbage".to_string()), "TEST", 64),
            64,
            "unparseable value must fall back to the default"
        );
    }

    /// Env parser (usize, ENTRIES): absent → default.
    #[test]
    fn test_resolve_env_usize_absent_falls_back() {
        assert_eq!(
            resolve_env_usize(None, "TEST", 64),
            64,
            "absent value must fall back to the default"
        );
    }
}
