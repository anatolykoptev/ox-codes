use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use tokio::sync::Semaphore;

/// Hard deadline for a single dataflow request. Note: `spawn_blocking` cannot
/// be cancelled — the task continues running in the blocking thread pool after
/// the timeout fires, but the HTTP response is returned immediately.
const DATAFLOW_TIMEOUT_SECS: u64 = 25;

/// Maximum concurrent directory walks. Each walk can be CPU/IO-heavy; capping
/// at 8 (2x the 4 ARM cores) prevents phantom tasks from saturating the Tokio
/// blocking pool (default 512 threads) during bursts of oversized-repo requests.
/// The permit is held until the blocking task finishes (even after a timeout),
/// so the pool is always bounded to this many active walks.
const SEMAPHORE_PERMITS: usize = 8;

/// Bounded timeout for acquiring a walk permit. If all permits are held by
/// in-flight (potentially stuck) walks, the caller fails fast with HTTP 503
/// instead of queueing forever in `.acquire().await` with no timeout.
const WALK_ACQUIRE_TIMEOUT_SECS: u64 = 5;

static WALK_SEMAPHORE: Semaphore = Semaphore::const_new(SEMAPHORE_PERMITS);

// ── Walk observability ───────────────────────────────────────────────────
//
// `WALK_METRICS` tracks how many walks are in-flight and the start timestamp
// of the oldest one. A walk whose in-flight duration exceeds
// `DATAFLOW_TIMEOUT_SECS` is "stuck" — its permit will never be returned
// because `spawn_blocking` cannot be cancelled. The staleness signal lets an
// operator detect a degraded pool (fewer and fewer available permits) before
// the pool is fully exhausted and every request starts getting 503.
//
// Approximation: `oldest_start_ms` is set when the first walk starts and
// cleared when the last walk finishes. If the first walk finishes but others
// remain, `oldest_start_ms` may hold a stale (finished) walk's timestamp,
// producing a false-positive staleness signal. This is acceptable for an
// observability signal — the operator investigates, not auto-scales.

static WALK_METRICS: WalkMetrics = WalkMetrics::new_const();

pub(crate) struct WalkMetrics {
    in_flight: AtomicU64,
    oldest_start_ms: AtomicU64,
}

impl WalkMetrics {
    const fn new_const() -> Self {
        Self {
            in_flight: AtomicU64::new(0),
            oldest_start_ms: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn new() -> Self {
        Self {
            in_flight: AtomicU64::new(0),
            oldest_start_ms: AtomicU64::new(0),
        }
    }

    fn acquire_slot(&self) -> WalkSlot<'_> {
        let now = now_ms();
        let prev = self.in_flight.fetch_add(1, Ordering::Relaxed);
        if prev == 0 {
            self.oldest_start_ms.store(now, Ordering::Relaxed);
        }
        WalkSlot { metrics: self }
    }

    fn stats(&self) -> (u64, u64) {
        (
            self.in_flight.load(Ordering::Relaxed),
            self.oldest_start_ms.load(Ordering::Relaxed),
        )
    }
}

/// RAII guard that increments `in_flight` on creation and decrements on drop.
/// Moved into the `spawn_blocking` closure so it is dropped when the walk
/// finishes (even on panic), not when the outer timeout fires.
pub(crate) struct WalkSlot<'a> {
    metrics: &'a WalkMetrics,
}

impl Drop for WalkSlot<'_> {
    fn drop(&mut self) {
        let prev = self.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            self.metrics.oldest_start_ms.store(0, Ordering::Relaxed);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Return (in_flight, oldest_start_ms) for the global walk metrics.
/// Exposed via `GET /cache/stats` → `walks` field.
pub(crate) fn walk_metrics() -> (u64, u64) {
    WALK_METRICS.stats()
}

/// Acquire a walk permit with a bounded timeout. On timeout, fail fast with
/// HTTP 503 (backpressure) instead of queueing forever.
async fn acquire_walk_permit(
    semaphore: &Semaphore,
    timeout: Duration,
) -> Result<tokio::sync::SemaphorePermit<'_>, (StatusCode, String)> {
    tokio::time::timeout(timeout, semaphore.acquire())
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "dataflow walk pool saturated; retry later".to_string(),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Run a blocking walk under the shared walk-pool guard.
///
/// Acquires a walk permit (→ HTTP 503 on `WALK_ACQUIRE_TIMEOUT_SECS`
/// saturation), tracks the walk for staleness observability, then runs `f`
/// inside `spawn_blocking` under the `DATAFLOW_TIMEOUT_SECS` request deadline
/// (→ HTTP 504). The permit AND the `WalkSlot` are moved into the blocking
/// closure so they are held until the closure finishes (even on panic or
/// outer timeout — `spawn_blocking` cannot be cancelled), preserving the
/// existing pool-bounding semantics exactly.
///
/// Error mapping: permit-acquire timeout → 503; request deadline → 504;
/// `JoinError` (panic) → 500; engine `Err` → 400.
///
/// Shared-budget contract — `WALK_SEMAPHORE` is a SINGLE process-global walk
/// budget (8 permits) shared across ALL endpoints that route through
/// `guarded_walk` (today: `/dataflow/analyze`; PR2/PR3 will add `/taint`,
/// `/structural`, …). This is the INTENDED single-pool design per the
/// architecture decision: a busy `/taint` or `/structural` burst CAN 503
/// `/dataflow/analyze` — there is NO per-endpoint isolation. Authors wiring
/// new callers in PR2/PR3 must NOT spin up a per-endpoint semaphore; reuse
/// this primitive so the whole process stays bounded to `SEMAPHORE_PERMITS`
/// concurrent walks.
pub(crate) async fn guarded_walk<T, F>(f: F) -> Result<T, (StatusCode, String)>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    // Thin wrapper over the parameterized inner — keeps the PRODUCTION
    // behavior byte-identical (same statics, same timeouts) while letting the
    // direct-seam test drive `guarded_walk_inner` with dedicated test-only
    // statics + tiny timeouts for deterministic 503/504/500/400 assertions.
    guarded_walk_inner(
        &WALK_SEMAPHORE,
        &WALK_METRICS,
        Duration::from_secs(WALK_ACQUIRE_TIMEOUT_SECS),
        Duration::from_secs(DATAFLOW_TIMEOUT_SECS),
        f,
    )
    .await
}

/// Parameterized core of [`guarded_walk`]. Exposed (crate-private) so the
/// direct-seam test can drive the REAL composition with dedicated test-only
/// `static Semaphore` + `static WalkMetrics` + tiny timeouts — making the
/// 503/504/500/400 outcomes deterministic regardless of parallel-test
/// interleaving on the module-global `WALK_SEMAPHORE`/`WALK_METRICS` and the
/// real 5s/25s timeouts. The body is verbatim the previous `guarded_walk`
/// body; the public wrapper preserves production behavior byte-identical.
pub(crate) async fn guarded_walk_inner<T, F>(
    sem: &'static Semaphore,
    metrics: &'static WalkMetrics,
    acquire_timeout: Duration,
    request_timeout: Duration,
    f: F,
) -> Result<T, (StatusCode, String)>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    // Acquire a walk permit with a bounded timeout. If all permits are held
    // by in-flight walks, fail fast with 503 instead of queueing forever.
    let permit = acquire_walk_permit(sem, acquire_timeout).await?;

    // Track the walk for staleness observability. The slot is moved into the
    // spawn_blocking closure so it is dropped when the walk finishes (even on
    // panic or timeout — the phantom task eventually completes and drops it).
    let slot = metrics.acquire_slot();

    let task = tokio::task::spawn_blocking(move || {
        // Keep the permit and slot alive until the blocking task finishes.
        // spawn_blocking cannot be cancelled, so even after the outer timeout
        // fires, this closure runs to completion and then drops both.
        let _slot = slot;
        let _permit = permit;
        f()
    });

    let result = tokio::time::timeout(request_timeout, task)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "dataflow analysis exceeded time limit".to_string(),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(result)
}

#[cfg(test)]
mod semaphore_tests {
    use super::*;
    use crate::dataflow::analyze_directory;
    use crate::dataflow_cache::DataflowCache;
    use ox_dataflow::DataflowInput;

    /// Verify the semaphore constant matches the design doc value (8 = 2x 4 ARM cores).
    #[test]
    fn semaphore_permits_is_8() {
        assert_eq!(SEMAPHORE_PERMITS, 8);
    }

    /// Verify that WALK_SEMAPHORE never exceeds SEMAPHORE_PERMITS available
    /// permits. Other concurrent tests may hold permits, so we assert `<=`
    /// rather than `==` (the global semaphore is shared across all tests).
    #[test]
    fn walk_semaphore_initial_permits() {
        assert!(
            WALK_SEMAPHORE.available_permits() <= SEMAPHORE_PERMITS,
            "available_permits {} should not exceed SEMAPHORE_PERMITS {}",
            WALK_SEMAPHORE.available_permits(),
            SEMAPHORE_PERMITS
        );
    }

    /// Acquire on a saturated semaphore must fail fast with HTTP 503
    /// (backpressure) instead of queueing forever. Reverting to an untimed
    /// `.acquire().await` makes this test hang (timeout → test failure).
    #[tokio::test]
    async fn acquire_walk_permit_returns_503_on_saturated_semaphore() {
        let sem = Semaphore::new(1);
        // Saturate the single permit.
        let _blocker = sem.acquire().await.unwrap();

        let result = acquire_walk_permit(&sem, Duration::from_millis(50)).await;
        assert!(
            result.is_err(),
            "saturated semaphore should return an error"
        );
        let (status, msg) = result.unwrap_err();
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "saturated pool should return 503"
        );
        assert!(
            msg.contains("saturated"),
            "error message should mention saturation: {msg}"
        );
    }

    /// Acquire on a semaphore with available permits succeeds normally.
    #[tokio::test]
    async fn acquire_walk_permit_succeeds_when_permits_available() {
        let sem = Semaphore::new(2);
        let permit = acquire_walk_permit(&sem, Duration::from_millis(100))
            .await
            .expect("permits available, should succeed");
        assert_eq!(sem.available_permits(), 1, "one permit should be held");
        drop(permit);
        assert_eq!(sem.available_permits(), 2, "permit released");
    }

    /// WalkMetrics tracks in-flight walk count and oldest start timestamp.
    /// Reverting WalkGuard (removing the increment/decrement) REDS this test.
    #[test]
    fn walk_metrics_tracks_in_flight() {
        let metrics = WalkMetrics::new();
        let (in_flight, oldest) = metrics.stats();
        assert_eq!(in_flight, 0);
        assert_eq!(oldest, 0);

        let slot1 = metrics.acquire_slot();
        let (in_flight, oldest) = metrics.stats();
        assert_eq!(in_flight, 1);
        assert!(oldest > 0, "oldest_start_ms should be set on first acquire");

        let slot2 = metrics.acquire_slot();
        let (in_flight, _) = metrics.stats();
        assert_eq!(in_flight, 2);

        drop(slot1);
        let (in_flight, _) = metrics.stats();
        assert_eq!(in_flight, 1);

        drop(slot2);
        let (in_flight, oldest) = metrics.stats();
        assert_eq!(in_flight, 0, "in_flight should return to 0");
        assert_eq!(
            oldest, 0,
            "oldest_start_ms should clear when no walks remain"
        );
    }

    /// Dedicated test-only statics for `concurrent_walks_through_real_path_drop_balances`.
    /// These are NOT shared with any other test (unlike `WALK_SEMAPHORE`/`WALK_METRICS`),
    /// so exact-count assertions on them are deterministic regardless of parallel
    /// test interleaving. `'static` references are required because `SemaphorePermit`
    /// and `WalkSlot` are moved into `spawn_blocking` closures.
    static TEST_SEM: Semaphore = Semaphore::const_new(4);
    static TEST_METRICS: WalkMetrics = WalkMetrics::new_const();

    /// Concurrency stress test through the REAL walk path:
    /// `acquire_walk_permit` + `WalkMetrics::acquire_slot` + `spawn_blocking`
    /// with the `WalkSlot` moved into the closure (so Drop runs on panic).
    ///
    /// Robustness approach — DEDICATED TEST-ONLY STATIC instances:
    /// `WALK_SEMAPHORE` and `WALK_METRICS` are process-global statics shared
    /// across parallel-running tests (this is why `walk_semaphore_initial_permits`
    /// had to weaken `==`→`<=`). To avoid flakiness, this test uses DEDICATED
    /// `static` items (`TEST_SEM`, `TEST_METRICS`) that NO other test touches —
    /// so exact-count assertions (in_flight → 0, Drop-balance) are deterministic
    /// regardless of parallel test interleaving. The REAL functions
    /// (`acquire_walk_permit`, `acquire_slot`, `WalkSlot::drop`) are exercised
    /// on `'static` references (required for `spawn_blocking`). No
    /// `serial_test` dep needed; the test is fully deterministic.
    ///
    /// Reverting any of the three mechanisms (untimed acquire, missing
    /// acquire_slot, or WalkSlot not moved into the closure) REDS this test:
    /// (a) untimed acquire → the 503 saturation sub-case hangs; (b) missing
    /// acquire_slot → in_flight stays 0, the return-to-zero assertion is
    /// vacuous; (c) WalkSlot not moved into the closure → the panic task
    /// drops the slot before the closure runs, in_flight decrements too early
    /// and the "return to 0 after all tasks" assertion can race.
    #[tokio::test]
    async fn concurrent_walks_through_real_path_drop_balances() {
        // ── Sub-case (a): N concurrent tasks through the real path, including
        // a panicking closure whose WalkSlot must still Drop (in_flight → 0).
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3u8 {
            let p = dir.path().join(format!("f{i}.ts"));
            std::fs::write(&p, b"const x = 1;").unwrap();
        }
        let root = dir.path().to_string_lossy().into_owned();

        let mut handles = Vec::new();
        for task_i in 0..8u8 {
            let r = root.clone();
            let h = tokio::spawn(async move {
                // Real path: bounded-timeout acquire (not raw .acquire().await).
                let permit = acquire_walk_permit(&TEST_SEM, Duration::from_secs(5))
                    .await
                    .expect("permits available for 8 tasks on a 4-permit sem");
                // Real path: acquire_slot, moved into spawn_blocking so Drop
                // runs when the closure finishes (even on panic).
                let slot = TEST_METRICS.acquire_slot();

                if task_i == 7 {
                    // Panicking closure: WalkSlot must still Drop during unwind.
                    let join_result = tokio::task::spawn_blocking(move || {
                        let _slot = slot;
                        let _permit = permit;
                        panic!("intentional panic in walk closure");
                    })
                    .await;
                    assert!(
                        join_result.is_err(),
                        "panic task should propagate as JoinError"
                    );
                    None
                } else {
                    let cache = DataflowCache::new();
                    let input = DataflowInput {
                        root: r,
                        language: "typescript".to_string(),
                        max_results: 100,
                        max_files: Some(10_000),
                        file_glob: None,
                        exclude_glob: None,
                    };
                    let resp = tokio::task::spawn_blocking(move || {
                        let _slot = slot;
                        let _permit = permit;
                        analyze_directory(input, &cache)
                    })
                    .await
                    .expect("task should not panic")
                    .expect("analysis should succeed");
                    Some(resp)
                }
            });
            handles.push(h);
        }

        let mut ok_completed = 0usize;
        let mut panic_completed = 0usize;
        for h in handles {
            match h.await.expect("outer task panicked") {
                Some(resp) => {
                    assert_eq!(resp.files_analyzed, 3);
                    ok_completed += 1;
                }
                None => panic_completed += 1,
            }
        }
        assert_eq!(ok_completed, 7, "7 non-panic tasks should complete");
        assert_eq!(panic_completed, 1, "1 panic task should report JoinError");

        // (b) After ALL tasks complete (including the panicking one), in_flight
        // must be back to 0 — guards a leaked WalkSlot. This is exact-count
        // safe because `TEST_METRICS` is a dedicated static no other test
        // touches (unlike the shared `WALK_METRICS`).
        let (in_flight, _) = TEST_METRICS.stats();
        assert_eq!(
            in_flight, 0,
            "in_flight must return to 0 after all tasks (including panic) complete; \
             a non-zero value means a WalkSlot leaked (Drop did not run)"
        );

        // ── Sub-case (c): under a saturated permit set, at least one acquire
        // returns 503 rather than hanging. Uses a local semaphore (no 'static
        // requirement — the permit is not moved into spawn_blocking here).
        let sem2 = Semaphore::new(1);
        let _blocker = sem2.acquire().await.unwrap(); // saturate the single permit
        let result = acquire_walk_permit(&sem2, Duration::from_millis(50)).await;
        assert!(
            result.is_err(),
            "saturated semaphore should return an error, not hang"
        );
        let (status, msg) = result.unwrap_err();
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "saturated pool should return 503"
        );
        assert!(
            msg.contains("saturated"),
            "error message should mention saturation: {msg}"
        );
    }
}

/// Direct-seam tests for [`guarded_walk_inner`] — the parameterized core of
/// `guarded_walk`. The moved concurrency test
/// `concurrent_walks_through_real_path_drop_balances` HAND-COPIES the
/// composition (calls `acquire_walk_permit` + `acquire_slot` + `spawn_blocking`
/// and re-composes them in its own closure) — it does NOT call `guarded_walk`
/// itself, so a composition regression (swapping the two `.map_err` arms, or
/// hoisting the permit/slot OUT of the `spawn_blocking` closure) would land
/// GREEN. These tests call the REAL seam (`guarded_walk_inner`) and assert the
/// four error outcomes + the happy path, closing that regression gap.
///
/// Determinism — each test gets its OWN dedicated `static Semaphore` +
/// `static WalkMetrics` pair (NOT the production `WALK_SEMAPHORE`/
/// `WALK_METRICS`, and NOT shared with each other). This matters because the
/// 504 sub-case's `spawn_blocking` closure cannot be cancelled — after the
/// 20ms request timeout fires the phantom task keeps sleeping for ~200ms and
/// keeps its `WalkSlot`/permit alive; if it shared `METRICS_OK` with the
/// happy-path test, the happy-path `in_flight == 0` assertion could race with
/// that lingering slot. Per-test statics make every assertion deterministic
/// regardless of parallel-test interleaving.
///
/// Revert-reds (verified by hand): swapping the two `.map_err` arms (so
/// `JoinError` → 400 and engine `Err` → 500) REDS both
/// `guarded_walk_500_on_closure_panic` (expects 500, would get 400) and
/// `guarded_walk_400_on_closure_err` (expects 400, would get 500). Hoisting
/// the `let slot = metrics.acquire_slot();` ABOVE the `acquire_walk_permit`
/// call (outside the spawn_blocking closure is already not the case, but
/// moving the permit acquire outside the closure) would break the
/// drop-balancing — covered by the existing concurrency test. The 503/504
/// outcomes RED if the corresponding `map_err` arm is removed or the timeout
/// is dropped.
#[cfg(test)]
mod guarded_walk_seam_tests {
    use super::*;

    // ── Per-test dedicated statics ───────────────────────────────────────
    // Single-permit sem for the 503 saturation case.
    static SEM_503: Semaphore = Semaphore::const_new(1);
    static METRICS_503: WalkMetrics = WalkMetrics::new_const();
    // Dedicated pair for the 504 case (its phantom task lingers ~200ms).
    static SEM_504: Semaphore = Semaphore::const_new(2);
    static METRICS_504: WalkMetrics = WalkMetrics::new_const();
    // Dedicated pair for the 500 panic case.
    static SEM_500: Semaphore = Semaphore::const_new(2);
    static METRICS_500: WalkMetrics = WalkMetrics::new_const();
    // Dedicated pair for the 400 err case.
    static SEM_400: Semaphore = Semaphore::const_new(2);
    static METRICS_400: WalkMetrics = WalkMetrics::new_const();
    // Dedicated pair for the happy-path case (in_flight==0 assertion).
    static SEM_OK: Semaphore = Semaphore::const_new(2);
    static METRICS_OK: WalkMetrics = WalkMetrics::new_const();

    /// 503: a saturated permit set must fail fast with SERVICE_UNAVAILABLE.
    /// REDS if the `acquire_walk_permit` 503 `map_err` arm is dropped (would
    /// hang/return INTERNAL_SERVER_ERROR instead).
    #[tokio::test]
    async fn guarded_walk_503_on_saturated_semaphore() {
        // Pre-acquire the only permit; hold it for the whole test so the
        // inner acquire must time out.
        let _blocker = SEM_503.acquire().await.unwrap();
        let result: Result<(), (StatusCode, String)> = guarded_walk_inner(
            &SEM_503,
            &METRICS_503,
            Duration::from_millis(50),
            Duration::from_secs(5),
            || Ok(()),
        )
        .await;
        let (status, msg) = result.expect_err("saturated pool should 503");
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "saturated pool should return 503"
        );
        assert!(
            msg.contains("saturated"),
            "error message should mention saturation: {msg}"
        );
        // No slot was acquired (acquire failed first), so in_flight stays 0.
        let (in_flight, _) = METRICS_503.stats();
        assert_eq!(in_flight, 0, "failed acquire must not acquire a slot");
    }

    /// 504: a closure that exceeds the request deadline must return
    /// GATEWAY_TIMEOUT. REDS if the outer `tokio::time::timeout` 504 `map_err`
    /// arm is dropped (would await the unbounded sleep instead).
    #[tokio::test]
    async fn guarded_walk_504_on_request_deadline_exceeded() {
        let result: Result<(), (StatusCode, String)> = guarded_walk_inner(
            &SEM_504,
            &METRICS_504,
            Duration::from_secs(5),
            Duration::from_millis(20),
            || {
                // spawn_blocking cannot be cancelled — this sleeps well past
                // the 20ms request timeout, holding its slot/permit in the
                // phantom task. We do NOT assert in_flight here (the slot is
                // still held when this test returns); the dedicated METRICS_OK
                // pair handles the drop-balance assertion.
                std::thread::sleep(Duration::from_millis(200));
                Ok(())
            },
        )
        .await;
        let (status, msg) = result.expect_err("slow closure should 504");
        assert_eq!(
            status,
            StatusCode::GATEWAY_TIMEOUT,
            "request deadline exceeded should return 504"
        );
        assert!(
            msg.contains("time limit"),
            "error message should mention the time limit: {msg}"
        );
    }

    /// 500: a panicking closure propagates as a JoinError → INTERNAL_SERVER_ERROR.
    /// REDS if the JoinError `map_err` arm is swapped onto the engine-Err arm
    /// (panic would then map to 400 instead of 500).
    #[tokio::test]
    async fn guarded_walk_500_on_closure_panic() {
        let result: Result<(), (StatusCode, String)> = guarded_walk_inner(
            &SEM_500,
            &METRICS_500,
            Duration::from_secs(5),
            Duration::from_secs(5),
            || panic!("intentional panic in guarded_walk closure"),
        )
        .await;
        let (status, _msg) = result.expect_err("panic should 500");
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "closure panic (JoinError) should return 500"
        );
    }

    /// 400: a closure returning `Err(anyhow!())` maps to BAD_REQUEST and the
    /// message is preserved. REDS if the engine-Err `map_err` arm is swapped
    /// onto the JoinError arm (engine Err would then map to 500 instead of 400).
    #[tokio::test]
    async fn guarded_walk_400_on_closure_err() {
        let result: Result<(), (StatusCode, String)> = guarded_walk_inner(
            &SEM_400,
            &METRICS_400,
            Duration::from_secs(5),
            Duration::from_secs(5),
            || Err(anyhow::anyhow!("boom")),
        )
        .await;
        let (status, msg) = result.expect_err("closure Err should 400");
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "engine Err should return 400"
        );
        assert!(
            msg.contains("boom"),
            "error message should preserve the engine error text: {msg}"
        );
    }

    /// Happy path: `Ok(v)` returns `Ok(v)` and `in_flight` returns to 0 after.
    /// REDS if the `Ok(result)` return is dropped/mis-mapped, or if the
    /// `WalkSlot` is not moved into the closure (in_flight would stay 1).
    #[tokio::test]
    async fn guarded_walk_ok_returns_value_and_balances_metrics() {
        let result: Result<u32, (StatusCode, String)> = guarded_walk_inner(
            &SEM_OK,
            &METRICS_OK,
            Duration::from_secs(5),
            Duration::from_secs(5),
            || Ok(42u32),
        )
        .await;
        assert_eq!(
            result.expect("happy path should return Ok(42)"),
            42,
            "Ok value must pass through unchanged"
        );
        let (in_flight, _) = METRICS_OK.stats();
        assert_eq!(
            in_flight, 0,
            "in_flight must return to 0 after the happy path; a non-zero value \
             means the WalkSlot was not moved into the spawn_blocking closure \
             (or was leaked)"
        );
    }
}
