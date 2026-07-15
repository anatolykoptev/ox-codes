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
pub const CACHE_ENTRIES_ENV: &str = "OX_CODES_DATAFLOW_CACHE_ENTRIES";

/// Environment variable override for the dataflow result cache entry TTL.
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
        let raw = std::env::var(CACHE_ENTRIES_ENV).ok();
        let capacity = match raw.as_deref().and_then(|s| s.parse::<usize>().ok()) {
            Some(0) => {
                tracing::warn!(
                    "{}=0 is a useless cache capacity; using default {}",
                    CACHE_ENTRIES_ENV,
                    DEFAULT_CAPACITY
                );
                DEFAULT_CAPACITY
            }
            Some(c) => c,
            None => {
                if let Some(ref value) = raw {
                    tracing::warn!(
                        "{}={:?} is unparseable; using default {}",
                        CACHE_ENTRIES_ENV,
                        value,
                        DEFAULT_CAPACITY
                    );
                }
                DEFAULT_CAPACITY
            }
        };
        let ttl_secs = parse_env_u64(CACHE_TTL_ENV, DEFAULT_TTL_SECS);
        Self::with_capacity_and_ttl(capacity, ttl_secs)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_ttl(capacity, DEFAULT_TTL_SECS)
    }

    fn with_capacity_and_ttl(capacity: usize, ttl_secs: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(capacity as u64)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

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
    match std::env::var(env).ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(0) => {
            tracing::warn!("{}=0 is invalid; using default {}", env, default);
            default
        }
        Some(v) => v,
        None => {
            if let Ok(raw) = std::env::var(env) {
                tracing::warn!(
                    "{}={:?} is unparseable; using default {}",
                    env,
                    raw,
                    default
                );
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
