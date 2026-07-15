use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use moka::sync::Cache;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use ox_langs::ScopeKind;

/// Default cap on the total cached source bytes.
///
/// This cap governs cached *source bytes* only; the `spans` Vec, key,
/// and moka bookkeeping overhead are extra.
const DEFAULT_CAPACITY_BYTES: u64 = 256 * 1024 * 1024;

/// Default TTL for parsed-scope entries.
///
/// The mtime+len fingerprint is cheap but can serve stale scopes for a
/// same-length in-place edit within the filesystem's mtime resolution. A moka
/// TTL bounds the worst-case staleness to this many seconds without the cost of
/// content-hashing every file.
const DEFAULT_TTL_SECS: u64 = 300;

/// Environment variable override for the scope cache byte limit.
pub const CACHE_BYTES_ENV: &str = "OX_CODES_SCOPE_CACHE_BYTES";

/// Environment variable override for the scope cache entry TTL.
pub const CACHE_TTL_ENV: &str = "OX_CODES_SCOPE_CACHE_TTL_SECS";

/// A single scope span extracted from a parsed tree-sitter tree.
/// `start` and `end` are byte offsets into `CachedScopes::source`.
/// `start_line` is 1-indexed.
#[derive(Debug, Clone)]
pub struct ScopeSpan {
    pub start: usize,
    pub end: usize,
    pub start_line: usize,
}

/// Parsed source and the scope spans within it.
/// Holds only `Send + Sync` bytes and ranges so it can live in a shared cache.
#[derive(Debug, Clone)]
pub struct CachedScopes {
    pub source: Arc<[u8]>,
    pub spans: Vec<ScopeSpan>,
}

/// Cache key for the parsed-scope cache.
///
/// Uses mtime + file length as a cheap fingerprint. A content hash would
/// require reading the file, defeating the skip-read win. mtime+len is the
/// standard build-cache fingerprint and is safe for an in-memory read cache.
///
/// A moka TTL further caps worst-case staleness (see `ScopeCache`) for the
/// rare case of a same-length in-place edit that lands within the filesystem's
/// mtime resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub canonical_abs_path: PathBuf,
    pub mtime_nanos: u128,
    pub file_len: u64,
    pub language: String,
    pub scope_kind: ScopeKind,
}

struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
}

/// Cross-request cache for parsed scopes.
///
/// Key: (canonical_abs_path, mtime_nanos, file_len, language, scope_kind).
/// Value: Arc<CachedScopes { source, spans }>.
///
/// Entries carry a moka TTL (default 300s, override via
/// `OX_CODES_SCOPE_CACHE_TTL_SECS`) that acts as a staleness backstop for the
/// mtime+len fingerprint without paying for a content hash on every read.
#[derive(Clone)]
pub struct ScopeCache {
    cache: Cache<CacheKey, Arc<CachedScopes>>,
    stats: Arc<CacheStats>,
}

impl ScopeCache {
    pub fn new() -> Self {
        let capacity = parse_env_u64(CACHE_BYTES_ENV, DEFAULT_CAPACITY_BYTES);
        let ttl_secs = parse_env_u64(CACHE_TTL_ENV, DEFAULT_TTL_SECS);
        Self::with_capacity_and_ttl(capacity, ttl_secs)
    }

    pub fn with_capacity(bytes: u64) -> Self {
        Self::with_capacity_and_ttl(bytes, DEFAULT_TTL_SECS)
    }

    fn with_capacity_and_ttl(bytes: u64, ttl_secs: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(bytes)
            .weigher(|_key, value: &Arc<CachedScopes>| {
                value.source.len().min(u32::MAX as usize) as u32
            })
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

        Self {
            cache,
            stats: Arc::new(CacheStats {
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
            }),
        }
    }

    /// Return (hits, misses) counters.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.stats.hits.load(Ordering::Relaxed),
            self.stats.misses.load(Ordering::Relaxed),
        )
    }

    /// Return the number of entries in the cache.
    ///
    /// Runs pending internal maintenance first so the count is accurate.
    pub fn entry_count(&self) -> u64 {
        self.cache.run_pending_tasks();
        self.cache.entry_count()
    }

    /// Get or insert the cached scopes for `key`.
    /// `init` is called on a miss and must read the file and parse it.
    /// Returns the value and a flag that is `true` on cache hit.
    pub fn get_or_insert<F>(&self, key: CacheKey, init: F) -> Result<(Arc<CachedScopes>, bool)>
    where
        F: FnOnce() -> Result<Arc<CachedScopes>>,
    {
        let was_miss = Arc::new(AtomicBool::new(false));
        let value = self.cache.try_get_with(key, {
            let was_miss = Arc::clone(&was_miss);
            move || {
                was_miss.store(true, Ordering::Relaxed);
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

    /// Parse source bytes and extract scope spans using the supplied query.
    pub fn parse_scopes(
        source: Vec<u8>,
        query: &Query,
        language: &Language,
    ) -> Result<CachedScopes> {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse failed"))?;

        let mut cursor = QueryCursor::new();
        let mut spans = Vec::new();
        let mut query_matches = cursor.matches(query, tree.root_node(), &*source);
        while let Some(qmatch) = query_matches.next() {
            for capture in qmatch.captures {
                let node = capture.node;
                spans.push(ScopeSpan {
                    start: node.byte_range().start,
                    end: node.byte_range().end,
                    start_line: node.start_position().row + 1,
                });
            }
        }

        let source = Arc::from(source.into_boxed_slice());
        Ok(CachedScopes { source, spans })
    }
}

impl Default for ScopeCache {
    fn default() -> Self {
        Self::new()
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
