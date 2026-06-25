//! The write-permit guard over `tokio::sync::Semaphore` (D14 / spine §4.2).
//!
//! Encapsulates the single-writer serialization so `write.rs` never juggles a raw
//! [`tokio::sync::OwnedSemaphorePermit`]. The `Session` owns one `Arc<Semaphore>` with **exactly one
//! permit**; every mutation holds a [`WriteGuard`] for the **entire** storage transaction, then
//! releases on drop — serializing all in-process writers (linearizable per FR-9). **Reads never
//! touch the permit** (FR-10).
//!
//! # Cancel-safety (spine §4.2, NORMATIVE)
//!
//! Permit acquisition is uncancel-safe across the tx boundary: a dropped future before commit
//! releases the permit (RAII on [`WriteGuard`]) and leaves the DB committed-or-rolled-back — the
//! libsql `BEGIN IMMEDIATE` tx either committed or rolled back, never partial (NFR-5). The guard
//! does **not** observe the tx outcome; it only guarantees the permit is freed and reusable.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::EngineError;

/// The number of write permits per `Session` — exactly one (D14, spine §4.2: single in-process
/// writer).
pub(crate) const WRITE_PERMITS: usize = 1;

/// An RAII guard that holds the single write permit across a storage transaction (D14).
///
/// Acquired via [`acquire_write`]; held for the whole mutation tx; releases the permit on drop
/// (including a drop caused by future cancellation — the cancel-safety contract, spine §4.2).
#[derive(Debug)]
pub(crate) struct WriteGuard(#[allow(dead_code)] OwnedSemaphorePermit);

/// Acquire the single write permit, shutdown-aware (spine §4.2).
///
/// Checks the cooperative shutdown flag **first** — if a shutdown is in progress the call fails fast
/// with [`EngineError::ShutdownInProgress`] (no new writes are accepted, FR-17) **before** parking on
/// the semaphore. Otherwise it `acquire_owned().await`s the single permit, parking until the current
/// holder releases. A closed semaphore (poisoned) surfaces [`EngineError::WritePermitPoisoned`].
///
/// # Errors
///
/// - [`EngineError::ShutdownInProgress`] if the shutdown flag is set when called.
/// - [`EngineError::WritePermitPoisoned`] if the semaphore was closed.
pub(crate) async fn acquire_write(
    permit: &Arc<Semaphore>,
    shutdown: &AtomicBool,
) -> Result<WriteGuard, EngineError> {
    // Shutdown is checked BEFORE parking on the semaphore (spine §4.2): a draining session refuses
    // new writers up front rather than letting them queue behind the in-flight one.
    if shutdown.load(Ordering::SeqCst) {
        return Err(EngineError::ShutdownInProgress);
    }
    let permit = Arc::clone(permit)
        .acquire_owned()
        .await
        .map_err(|_closed| EngineError::WritePermitPoisoned)?;
    Ok(WriteGuard(permit))
}

#[cfg(test)]
mod tests {
    use super::{WRITE_PERMITS, acquire_write};
    use crate::error::EngineError;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::sync::Semaphore;

    fn permit() -> Arc<Semaphore> {
        Arc::new(Semaphore::new(WRITE_PERMITS))
    }

    #[tokio::test]
    async fn single_permit_serializes_second_acquire() {
        let sem = permit();
        let flag = AtomicBool::new(false);

        // First guard holds the only permit.
        let g1 = acquire_write(&sem, &flag).await.expect("first acquires");
        assert_eq!(sem.available_permits(), 0);

        // A second acquire must PARK (it cannot complete while g1 is alive).
        let parked = acquire_write(&sem, &flag);
        let race = tokio::time::timeout(Duration::from_millis(50), parked).await;
        assert!(
            race.is_err(),
            "second acquire must park while the first holds"
        );

        // Dropping g1 frees the permit; the second acquire can now succeed.
        drop(g1);
        let g2 = acquire_write(&sem, &flag)
            .await
            .expect("acquires after first drops");
        assert_eq!(sem.available_permits(), 0);
        drop(g2);
        assert_eq!(sem.available_permits(), WRITE_PERMITS);
    }

    #[tokio::test]
    async fn acquire_after_shutdown_flag_set_fails_fast() {
        let sem = permit();
        let flag = AtomicBool::new(true); // shutdown already requested.
        let err = acquire_write(&sem, &flag).await.expect_err("must refuse");
        assert!(matches!(err, EngineError::ShutdownInProgress));
        // The permit was NOT consumed (we never parked on the semaphore).
        assert_eq!(sem.available_permits(), WRITE_PERMITS);
    }

    #[tokio::test]
    async fn drop_mid_tx_releases_permit_for_reuse() {
        // Model a "parked mid-tx" future that is dropped (cancelled) before completing: the guard's
        // RAII drop must return the permit so the next writer can proceed (cancel-safety, spine §4.2).
        let sem = permit();
        let flag = AtomicBool::new(false);

        let acquired = {
            let g = acquire_write(&sem, &flag).await.expect("acquire");
            assert_eq!(sem.available_permits(), 0);
            // Simulate the future being cancelled mid-tx by dropping the guard here.
            drop(g);
            sem.available_permits()
        };
        assert_eq!(acquired, WRITE_PERMITS, "permit reusable after mid-tx drop");

        // The next writer acquires cleanly.
        let g2 = acquire_write(&sem, &flag).await.expect("reacquire");
        drop(g2);
    }

    #[tokio::test]
    async fn poisoned_semaphore_surfaces_typed_error() {
        let sem = permit();
        let flag = AtomicBool::new(false);
        sem.close();
        let err = acquire_write(&sem, &flag).await.expect_err("closed");
        assert!(matches!(err, EngineError::WritePermitPoisoned));
        // Sanity: the flag was not the cause.
        assert!(!flag.load(Ordering::SeqCst));
    }
}
