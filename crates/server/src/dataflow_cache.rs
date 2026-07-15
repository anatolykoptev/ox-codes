use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use moka::sync::Cache;
use ox_dataflow::DataflowResponse;

/// Default cap on the number of cached repo results.
/// A single `DataflowResponse` is one-per-repo and can be larger/variable than a
/// parsed-scope entry, so this cache is bounded by entry count rather than bytes.
const DEFAULT_CAPACITY: usize = 64;

/// Environment variable override for the dataflow result cache entry limit.
pub const CACHE_ENTRIES_ENV: &str = "OX_CODES_DATAFLOW_CACHE_ENTRIES";

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
#[derive(Clone)]
pub struct DataflowCache {
    cache: Cache<DataflowCacheKey, Arc<DataflowResponse>>,
    stats: Arc<CacheStats>,
}

impl DataflowCache {
    pub fn new() -> Self {
        let capacity = std::env::var(CACHE_ENTRIES_ENV)
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_CAPACITY);
        Self::with_capacity(capacity)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let cache = Cache::builder().max_capacity(capacity as u64).build();

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

    /// Get or insert the cached result for `key`.
    /// `init` is called on a miss and must run the full analysis.
    /// Errors are NOT cached.
    pub fn get_or_insert<F>(&self, key: DataflowCacheKey, init: F) -> Result<Arc<DataflowResponse>>
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
                if was_miss.load(Ordering::Relaxed) {
                    self.stats.misses.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.hits.fetch_add(1, Ordering::Relaxed);
                }
                Ok(v)
            }
            Err(e) => {
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                Err(anyhow::anyhow!("{e}"))
            }
        }
    }
}

impl Default for DataflowCache {
    fn default() -> Self {
        Self::new()
    }
}
