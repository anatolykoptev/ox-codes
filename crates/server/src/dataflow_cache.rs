use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use ox_core::weighted_cache::{WeightedEnvCache, parse_env_u64, resolve_env_u64};
use ox_dataflow::{DataflowResponse, Finding};

/// Default cap on the total cached response bytes.
///
/// This cap governs cached *response* bytes only (estimated from finding
/// string lengths); the cache key, moka bookkeeping overhead, and Arc
/// pointer itself are extra. Mirrors the byte-weighed discipline of
/// `ScopeCache` (`crates/core/src/scope_cache.rs`).
const DEFAULT_CAPACITY_BYTES: u64 = 64 * 1024 * 1024;

/// Default TTL for dataflow result entries.
///
/// The aggregate fingerprint is mtime+len per file, which can serve stale results
/// for same-length in-place edits within the filesystem's mtime resolution. A
/// moka TTL bounds the worst-case staleness to this many seconds without paying
/// for a content hash of every file.
const DEFAULT_TTL_SECS: u64 = 300;

/// Environment variable override for the dataflow result cache byte limit.
///
/// `0` = disable the cache (`max_capacity(0)`, every entry evicted immediately
/// → every request a miss — a real kill-switch). Absent/unparseable → default.
pub const CACHE_BYTES_ENV: &str = "OX_CODES_DATAFLOW_CACHE_BYTES";

/// Legacy env var name (pre-rename). `OX_CODES_DATAFLOW_CACHE_ENTRIES` was the
/// entry-count cap; it was renamed to `OX_CODES_DATAFLOW_CACHE_BYTES` (byte
/// ceiling) with different semantics. If only the OLD name is set, we warn
/// loudly and do NOT silently reinterpret the old numeric value as bytes (a
/// value like 64 would mean a 64-BYTE cache = effectively disabled). One-
/// release deprecation shim.
pub const CACHE_ENTRIES_LEGACY_ENV: &str = "OX_CODES_DATAFLOW_CACHE_ENTRIES";

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

/// Cross-request cache for whole-repo dataflow analysis results.
///
/// Key: repo root + language + include/exclude globs + max_results + an aggregate
/// fingerprint of the analyzed file set (mtime_nanos + file_len per file).
/// Value: `Arc<DataflowResponse>`.
///
/// Entries carry a moka TTL (default 300s, override via
/// `OX_CODES_DATAFLOW_CACHE_TTL_SECS`) that acts as a staleness backstop for the
/// mtime+len fingerprint without paying for a content hash of every file.
///
/// A thin wrapper over [`WeightedEnvCache`] — all byte-weighted capacity / TTL
/// / env-parsing / kill-switch / hit-miss logic lives in the shared generic.
/// The only dataflow-specific pieces are the `estimate_response_bytes` weigher
/// and the `OX_CODES_DATAFLOW_CACHE_ENTRIES` legacy deprecation shim.
#[derive(Clone)]
pub struct DataflowCache {
    inner: WeightedEnvCache<DataflowCacheKey, Arc<DataflowResponse>>,
}

impl DataflowCache {
    pub fn new() -> Self {
        let bytes = resolve_cache_bytes_env(
            std::env::var(CACHE_BYTES_ENV).ok(),
            std::env::var(CACHE_ENTRIES_LEGACY_ENV).ok(),
        );
        let ttl_secs = parse_env_u64(CACHE_TTL_ENV, DEFAULT_TTL_SECS);
        Self::with_capacity_and_ttl(bytes, ttl_secs)
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
                "cache",
                "result",
                |_k, v: &Arc<DataflowResponse>| {
                    estimate_response_bytes(v).min(u32::MAX as usize) as u32
                },
            ),
        }
    }

    /// Return (hits, misses, analysis-invocation) counters.
    pub fn stats(&self) -> (u64, u64, u64) {
        self.inner.stats()
    }

    /// Return the number of entries in the cache.
    ///
    /// Runs pending internal maintenance first so the count is accurate.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
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
        self.inner.get_or_insert(key, init)
    }
}

/// Resolve the cache byte ceiling from the env, with a one-release deprecation
/// shim for the renamed `OX_CODES_DATAFLOW_CACHE_ENTRIES` →
/// `OX_CODES_DATAFLOW_CACHE_BYTES`.
///
/// Semantics:
/// - If the NEW env (`CACHE_BYTES_ENV`) is set and parseable (including `0`),
///   use it — same rules as `resolve_env_u64`.
/// - If the NEW env is set but unparseable, warn + fall back to the default.
/// - If the NEW env is UNSET but the OLD env (`CACHE_ENTRIES_LEGACY_ENV`) IS
///   set, emit a loud `tracing::warn!` that the var was renamed AND its
///   semantics changed (now BYTES, not entry count), and do NOT silently
///   reinterpret the old numeric value as bytes (e.g. 64 entries → 64 bytes
///   would effectively disable the cache). Fall back to the byte default.
/// - If both are unset, use the byte default silently.
///
/// Separated from the `std::env::var` read so it is unit-testable without
/// touching the process-global environment.
fn resolve_cache_bytes_env(new_raw: Option<String>, old_raw: Option<String>) -> u64 {
    // New env is set (parseable or not) — resolve it, ignoring the legacy name.
    if new_raw.is_some() {
        return resolve_env_u64(new_raw, CACHE_BYTES_ENV, DEFAULT_CAPACITY_BYTES);
    }
    // New env is UNSET. Check the legacy entry-count name.
    if old_raw.is_some() {
        tracing::warn!(
            legacy = CACHE_ENTRIES_LEGACY_ENV,
            new = CACHE_BYTES_ENV,
            "{} has been renamed to {} and its semantics changed (now BYTES, not entry count). \
             The old value is NOT being reinterpreted as bytes — using byte default {} ({} MB). \
             Update your env to {} and set a byte value (e.g. {} for a 64 MB ceiling).",
            CACHE_ENTRIES_LEGACY_ENV,
            CACHE_BYTES_ENV,
            DEFAULT_CAPACITY_BYTES,
            DEFAULT_CAPACITY_BYTES / (1024 * 1024),
            CACHE_BYTES_ENV,
            DEFAULT_CAPACITY_BYTES,
        );
        // Do NOT reinterpret old_raw as bytes — fall back to the byte default.
        return DEFAULT_CAPACITY_BYTES;
    }
    // Both unset — silent default.
    DEFAULT_CAPACITY_BYTES
}

/// Estimate the retained byte size of a `DataflowResponse` for the cache
/// weigher. This is a CONSERVATIVE OVER-APPROXIMATION — it intentionally
/// over-counts so the byte ceiling is not silently exceeded by uncounted
/// overhead (Vec spare capacity, String heap slack, enum/struct padding,
/// Arc/moka bookkeeping). The cap stays on the safe side: it is better to
/// evict a entry too early than to hold more bytes than the label promises.
///
/// Per-finding: the full `size_of::<Finding>()` footprint (stack slot in the
/// findings Vec, including String headers) PLUS the heap capacity of each
/// String field (`capacity() >= len()`, accounting for spare allocation).
/// The findings Vec itself is sized by `capacity()` (>= `len()`), so spare
/// slots are counted too. A rough estimate, just biased safe.
fn estimate_response_bytes(resp: &DataflowResponse) -> usize {
    // Base: usize fields (total_findings, files_analyzed, duration_ms) + 2
    // bools + Vec header + slack for Arc/moka bookkeeping not counted per-entry.
    let mut bytes = 128;
    // Findings Vec: count by capacity (>= len) to include spare slots.
    bytes += resp.findings.capacity() * std::mem::size_of::<Finding>();
    // Heap allocations for each String field: capacity >= len.
    for f in &resp.findings {
        bytes += f.message.capacity() + f.file.capacity() + f.variable.capacity();
    }
    bytes
}

impl Default for DataflowCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_dataflow::{Finding, FindingKind, Severity, Span};

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

    fn make_large_value(tag: &str) -> Arc<DataflowResponse> {
        Arc::new(DataflowResponse {
            findings: vec![Finding {
                kind: FindingKind::DeadStore,
                severity: Severity::Info,
                message: "x".repeat(1000),
                file: format!("/{tag}.ts"),
                span: Span {
                    start_byte: 0,
                    end_byte: 1,
                    start_line: 1,
                    end_line: 1,
                },
                variable: "x".into(),
            }],
            total_findings: 1,
            files_analyzed: 1,
            truncated: false,
            files_truncated: false,
            duration_ms: 0,
            is_hit: false,
        })
    }

    /// Cache eviction must be driven by byte weight, not entry count. Two
    /// entries whose combined byte weight exceeds the ceiling must evict the
    /// older one — even though the entry count (2) is far below the old
    /// entry-count cap (64). Reverting to a no-weigher cache (bounded by
    /// entry count) REDS this test: both entries survive.
    ///
    /// Dataflow-specific: exercises the `estimate_response_bytes` weigher
    /// (conservative over-approximation from #60) against real
    /// `DataflowResponse` values. The shared byte-weight-eviction guarantee
    /// itself is covered generically in `weighted_cache::tests`.
    #[test]
    fn test_cache_evicts_by_byte_weight() {
        // Ceiling large enough for one large entry but NOT two.
        // Each large entry weighs ~1150+ bytes (1000-byte message + overhead).
        let cache = DataflowCache::with_capacity_and_ttl(1500, 0);

        let key1 = make_key("byte-1");
        let key2 = make_key("byte-2");
        let val1 = make_large_value("byte-1");
        let val2 = make_large_value("byte-2");

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

    /// Legacy env-var rename shim: if the NEW `CACHE_BYTES_ENV` is unset but
    /// the OLD `CACHE_ENTRIES_LEGACY_ENV` IS set, the old numeric value must
    /// NOT be silently reinterpreted as bytes (e.g. 64 entries → 64 bytes
    /// would disable the cache). Instead, fall back to the byte default.
    /// Reverting the shim (passing the old value through as bytes) REDS this
    /// test: the result would be 64, not the default.
    #[test]
    fn test_legacy_entries_env_not_reinterpreted_as_bytes() {
        let result = resolve_cache_bytes_env(None, Some("64".to_string()));
        assert_eq!(
            result, DEFAULT_CAPACITY_BYTES,
            "legacy entry-count value must NOT be reinterpreted as bytes; \
             should fall back to the byte default"
        );
    }

    /// Legacy env-var rename shim: if the NEW env is set, the OLD env is
    /// ignored entirely (the new name takes precedence).
    #[test]
    fn test_new_env_takes_precedence_over_legacy() {
        let result = resolve_cache_bytes_env(
            Some("134217728".to_string()), // 128 MB in bytes
            Some("64".to_string()),        // legacy entry count — ignored
        );
        assert_eq!(
            result, 134_217_728,
            "new BYTES env must take precedence over the legacy ENTRIES env"
        );
    }

    /// Legacy env-var rename shim: both unset → silent byte default.
    #[test]
    fn test_both_envs_unset_uses_byte_default() {
        let result = resolve_cache_bytes_env(None, None);
        assert_eq!(
            result, DEFAULT_CAPACITY_BYTES,
            "both envs unset should use the byte default"
        );
    }

    /// Legacy env-var rename shim: new env `0` (kill-switch) is preserved
    /// even when the legacy env is also set.
    #[test]
    fn test_new_env_zero_preserved_with_legacy_set() {
        let result = resolve_cache_bytes_env(Some("0".to_string()), Some("64".to_string()));
        assert_eq!(
            result, 0,
            "new env 0 (kill-switch) must be preserved; legacy env ignored"
        );
    }
}
