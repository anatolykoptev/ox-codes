use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use moka::sync::Cache;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use ox_langs::ScopeKind;

/// Default cap on the total cached source bytes.
const DEFAULT_CAPACITY_BYTES: u64 = 256 * 1024 * 1024;

/// Environment variable override for the scope cache byte limit.
pub const CACHE_BYTES_ENV: &str = "OX_CODES_SCOPE_CACHE_BYTES";

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
/// Uses mtime + file length as a cheap fingerprint. A content hash would
/// require reading the file, defeating the skip-read win. mtime+len is the
/// standard build-cache fingerprint and is safe for an in-memory read cache.
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
#[derive(Clone)]
pub struct ScopeCache {
    cache: Cache<CacheKey, Arc<CachedScopes>>,
    stats: Arc<CacheStats>,
}

impl ScopeCache {
    pub fn new() -> Self {
        let capacity = std::env::var(CACHE_BYTES_ENV)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_CAPACITY_BYTES);
        Self::with_capacity(capacity)
    }

    pub fn with_capacity(bytes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(bytes)
            .weigher(|_key, value: &Arc<CachedScopes>| {
                value.source.len().min(u32::MAX as usize) as u32
            })
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

    /// Get or insert the cached scopes for `key`.
    /// `init` is called on a miss and must read the file and parse it.
    pub fn get_or_insert<F>(&self, key: CacheKey, init: F) -> Result<Arc<CachedScopes>>
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
