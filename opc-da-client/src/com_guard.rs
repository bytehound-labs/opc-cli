//! RAII guard for COM initialization/teardown.
//!
//! Ensures `CoUninitialize` is called exactly once per successful
//! `CoInitializeEx`, even on early returns or panics.

use std::marker::PhantomData;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// Drop guard for COM thread initialization.
///
/// Calling [`ComGuard::new`] initializes COM in Multi-Threaded Apartment
/// (MTA) mode. When the guard is dropped, `CoUninitialize` is called
/// automatically.
///
/// # Thread Safety
///
/// `ComGuard` is intentionally `!Send` and `!Sync`. COM initialization
/// is per-thread — the guard **must** be created and dropped on the same
/// OS thread. This is enforced at compile time.
///
/// # Examples
///
/// ```ignore
/// # use anyhow::Result;
/// # use crate::ComGuard;
/// # fn main() -> Result<()> {
/// let _guard = ComGuard::new()?;
/// // ... COM operations ...
/// // CoUninitialize called automatically on drop
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct ComGuard {
    /// Prevents `Send + Sync` auto-derivation. COM init is per-thread.
    _not_send: PhantomData<*mut ()>,
}

impl ComGuard {
    /// Initialize COM in Multi-Threaded Apartment (MTA) mode.
    ///
    /// Returns `Ok(ComGuard)` on success (including `S_FALSE`, which
    /// means COM was already initialized on this thread).
    ///
    /// # Errors
    ///
    /// Returns `Err` if `CoInitializeEx` fails with a fatal HRESULT.
    pub fn new() -> anyhow::Result<Self> {
        // SAFETY: CoInitializeEx is a standard Win32 FFI call passing COINIT_MULTITHREADED to join MTA.
        // SAFETY: Result is checked below, and CoUninitialize is guaranteed via Drop.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

        if let Err(e) = hr.ok() {
            tracing::error!(error = ?e, "COM MTA initialization failed");
            return Err(anyhow::anyhow!("CoInitializeEx failed: {e}"));
        }

        tracing::debug!("COM MTA initialized");

        Ok(Self {
            _not_send: PhantomData,
        })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        tracing::debug!("COM MTA teardown");
        // SAFETY: Paired with the successful CoInitializeEx in new().
        // SAFETY: Construction guarantees COM was initialized, so this call is always balanced. Only runs on the creating thread (!Send).
        unsafe {
            CoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn com_guard_constructs_and_drops() {
        // On Windows, CoInitializeEx(MTA) should succeed.
        // On non-Windows CI, this test is skipped by target gate.
        let guard = ComGuard::new();
        assert!(guard.is_ok(), "ComGuard::new() should succeed: {guard:?}");
        // Guard drops here — CoUninitialize runs.
    }
}
