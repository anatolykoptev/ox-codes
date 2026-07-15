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
        // ttl_secs=0 means "no TTL" (an explicit escape hatch): do not set
        // `time_to_live` at all, so a hot unchanged entry never expires.
        // Any other value bounds worst-case staleness to that many seconds.
        if let Some(msg) = ttl_zero_warning(ttl_secs) {
            tracing::warn!("{}", msg);
        }
        let mut builder =
            Cache::builder()
                .max_capacity(bytes)
                .weigher(|_key, value: &Arc<CachedScopes>| {
                    value.source.len().min(u32::MAX as usize) as u32
                });
        if ttl_secs > 0 {
            builder = builder.time_to_live(Duration::from_secs(ttl_secs));
        }
        let cache = builder.build();

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
    resolve_env_u64(std::env::var(env).ok(), env, default)
}

/// Pure helper: return a staleness-backstop-disabled warning message iff
/// `ttl_secs == 0` (the never-expire escape hatch), else `None`.
///
/// `TTL=0` inverts the near-universal ops convention (`Cache-Control: max-age=0`
/// = expire immediately) — here it means *never* expire. Combined with the
/// mtime+len fingerprint, a same-length in-place edit within mtime resolution
/// can serve a stale scope until restart. This warning makes the footgun
/// loud without changing the intentional behavior.
///
/// Separated from the `tracing::warn!` call so it is unit-testable without a
/// tracing subscriber (mirrors the `resolve_env_u64` pattern).
fn ttl_zero_warning(ttl_secs: u64) -> Option<String> {
    if ttl_secs == 0 {
        Some(format!(
            "{}=0: scope cache staleness backstop is DISABLED (TTL=0 means never-expire here, \
             the opposite of the max-age=0 convention). Combined with the mtime+len \
             fingerprint, a same-length in-place edit within mtime resolution can serve \
             a stale scope until restart. Set a non-zero TTL to re-enable the backstop.",
            CACHE_TTL_ENV,
        ))
    } else {
        None
    }
}

/// Pure resolution of a (possibly absent/unparseable) env value, separated from
/// the `std::env::var` read so it is unit-testable without touching the
/// process-global environment.
///
/// Env-var semantics (apply to BYTES and TTL alike):
/// - explicit `0` is a valid, intentional value — TTL=0 = no expiry,
///   BYTES=0 = disable the cache (`max_capacity(0)`). It is NOT remapped to the
///   default and does NOT warn.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(tag: &str) -> CacheKey {
        CacheKey {
            canonical_abs_path: PathBuf::from(format!("/nonexistent/{tag}")),
            mtime_nanos: 1,
            file_len: 1,
            language: "go".to_string(),
            scope_kind: ScopeKind::FunctionBodies,
        }
    }

    fn make_value() -> Arc<CachedScopes> {
        Arc::new(CachedScopes {
            source: Arc::from(b"hello".as_slice()),
            spans: Vec::new(),
        })
    }

    /// TTL=0 means "no expiry": a hot entry survives a re-get.
    /// Reverting to `time_to_live(Duration::from_secs(0))` makes moka expire the
    /// entry immediately, so the re-get becomes a miss and this test REDS.
    #[test]
    fn test_ttl_zero_means_no_expiry() {
        let cache = ScopeCache::with_capacity_and_ttl(64 * 1024 * 1024, 0);
        let key = make_key("ttl-zero");
        let val = make_value();

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
        let cache = ScopeCache::with_capacity_and_ttl(0, DEFAULT_TTL_SECS);
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

    /// Env parser: an explicit "0" is a valid intentional value (the
    /// TTL=0/BYTES=0 escape hatch), NOT remapped to the default.
    /// Reverting to the old `0 → default` mapping REDS this test.
    #[test]
    fn test_resolve_env_u64_zero_is_valid() {
        assert_eq!(
            resolve_env_u64(Some("0".to_string()), "TEST", 300),
            0,
            "explicit 0 must be preserved, not remapped to the default"
        );
    }

    /// Env parser: an unparseable value falls back to the default.
    #[test]
    fn test_resolve_env_u64_unparseable_falls_back() {
        assert_eq!(
            resolve_env_u64(Some("garbage".to_string()), "TEST", 300),
            300,
            "unparseable value must fall back to the default"
        );
    }

    /// Env parser: an absent value falls back to the default.
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
            ttl_zero_warning(0).is_some(),
            "ttl=0 must produce a staleness-backstop-disabled warning"
        );
    }

    /// Any non-zero TTL keeps the staleness backstop active — no warning.
    /// Reverting the helper (always returning Some) REDS this test.
    #[test]
    fn test_ttl_zero_warning_returns_none_for_nonzero() {
        assert!(ttl_zero_warning(300).is_none(), "default ttl must not warn");
        assert!(ttl_zero_warning(1).is_none(), "ttl=1 must not warn");
    }
}
