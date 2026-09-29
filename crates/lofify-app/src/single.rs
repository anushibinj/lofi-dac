//! Single-instance guard: a second copy of Lofify must not fight the first over the audio
//! devices, the tray icon and the hotkey.

/// Held for the life of the process. Dropping it releases the lock.
pub struct InstanceGuard {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    _inner: single_instance::SingleInstance,
}

impl std::fmt::Debug for InstanceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceGuard").finish_non_exhaustive()
    }
}

/// Returns `None` if another Lofify is already running.
pub fn acquire() -> Option<InstanceGuard> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        match single_instance::SingleInstance::new("io.github.lofify.app") {
            Ok(inner) if inner.is_single() => Some(InstanceGuard { _inner: inner }),
            Ok(_) => None,
            // If the lock itself cannot be created, prefer running over refusing to start.
            Err(e) => {
                tracing::warn!("single-instance lock unavailable: {e}");
                Some(InstanceGuard {
                    _inner: fallback()?,
                })
            }
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Some(InstanceGuard {})
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn fallback() -> Option<single_instance::SingleInstance> {
    // A per-process name always succeeds and never conflicts.
    single_instance::SingleInstance::new(&format!("io.github.lofify.app.{}", std::process::id()))
        .ok()
}
