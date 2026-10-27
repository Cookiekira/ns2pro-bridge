//! Thread-bound `WinRT` apartment and best-effort MMCSS registration.
use std::{marker::PhantomData, rc::Rc};
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::{
            Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
    },
    core::w,
};

#[derive(Debug)]
pub struct RuntimeGuard {
    mmcss: Option<HANDLE>,
    // Both registrations must be released on the registering runtime thread.
    _thread_bound: PhantomData<Rc<()>>,
}
impl RuntimeGuard {
    /// Joins the multithreaded `WinRT` apartment and requests MMCSS "Games"
    /// scheduling; MMCSS failure is logged and ignored.
    ///
    /// # Errors
    ///
    /// When `RoInitialize` fails.
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: balanced by Drop on this same thread; MTA permits WinRT callbacks.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        let mut task = 0;
        // SAFETY: the task name is a static NUL-terminated string and task is writable
        // u32 storage for the call; the handle is reverted in Drop on this thread.
        let mmcss = match unsafe { AvSetMmThreadCharacteristicsW(w!("Games"), &raw mut task) } {
            Ok(handle) => Some(handle),
            Err(error) => {
                tracing::debug!(%error, "MMCSS Games registration unavailable");
                None
            }
        };
        Ok(Self {
            mmcss,
            _thread_bound: PhantomData,
        })
    }
}
impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.mmcss.take() {
            // SAFETY: handle came from AvSetMmThreadCharacteristicsW on this thread (the
            // guard is !Send) and take() ensures it is reverted once.
            let result = unsafe { AvRevertMmThreadCharacteristics(handle) };
            if let Err(error) = result {
                tracing::debug!(%error, "MMCSS Games release failed");
            }
        }
        // SAFETY: balances the successful RoInitialize in new() on the same thread.
        unsafe { RoUninitialize() };
    }
}
