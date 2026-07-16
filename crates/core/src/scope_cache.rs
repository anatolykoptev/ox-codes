use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use ox_langs::ScopeKind;

use crate::weighted_cache::{WeightedEnvCache, parse_env_u64};

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
///
/// `0` = disable the cache (`max_capacity(0)`, every entry evicted immediately
/// → every request a miss — a real kill-switch). Absent/unparseable → default.
pub const CACHE_BYTES_ENV: &str = "OX_CODES_SCOPE_CACHE_BYTES";

/// Environment variable override for the scope cache entry TTL.
///
/// `0` = no TTL (do not set `time_to_live` — a hot unchanged entry never
/// expires). Absent/unparseable → default (300s).
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

/// Cross-request cache for parsed scopes.
///
/// Key: (canonical_abs_path, mtime_nanos, file_len, language, scope_kind).
/// Value: Arc<CachedScopes { source, spans }>.
///
/// Entries carry a moka TTL (default 300s, override via
/// `OX_CODES_SCOPE_CACHE_TTL_SECS`) that acts as a staleness backstop for the
/// mtime+len fingerprint without paying for a content hash on every read.
///
/// A thin wrapper over [`WeightedEnvCache`] — all byte-weighted capacity / TTL
/// / env-parsing / kill-switch / hit-miss logic lives in the shared generic.
#[derive(Clone)]
pub struct ScopeCache {
    inner: WeightedEnvCache<CacheKey, Arc<CachedScopes>>,
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
        Self {
            inner: WeightedEnvCache::with_capacity_and_ttl(
                bytes,
                ttl_secs,
                CACHE_TTL_ENV,
                "scope cache",
                "scope",
                |_key, value: &Arc<CachedScopes>| value.source.len().min(u32::MAX as usize) as u32,
            ),
        }
    }

    /// Return (hits, misses) counters.
    pub fn stats(&self) -> (u64, u64) {
        let (hits, misses, _analyses) = self.inner.stats();
        (hits, misses)
    }

    /// Return the number of entries in the cache.
    ///
    /// Runs pending internal maintenance first so the count is accurate.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Get or insert the cached scopes for `key`.
    /// `init` is called on a miss and must read the file and parse it.
    /// Returns the value and a flag that is `true` on cache hit.
    pub fn get_or_insert<F>(&self, key: CacheKey, init: F) -> Result<(Arc<CachedScopes>, bool)>
    where
        F: FnOnce() -> Result<Arc<CachedScopes>>,
    {
        self.inner.get_or_insert(key, init)
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
