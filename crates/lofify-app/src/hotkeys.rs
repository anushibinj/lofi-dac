//! The global bypass hotkey (default Ctrl+Alt+L).
//!
//! Real implementation on Windows and macOS (`global-hotkey`); a no-op stub elsewhere.
//! **UNTESTED on real desktops** (compile-checked only).

use crate::commands::CommandSender;
use crate::hotkey::Hotkey;

/// Owns the OS registration. Dropping it unregisters the hotkey.
pub struct HotkeyRegistration {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    imp: imp::Inner,
}

impl std::fmt::Debug for HotkeyRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotkeyRegistration").finish_non_exhaustive()
    }
}

impl HotkeyRegistration {
    /// Creates the manager. Must be called on the main thread (Windows needs its message loop).
    pub fn new(ctx: &eframe::egui::Context, tx: CommandSender) -> Option<HotkeyRegistration> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            match imp::Inner::new(ctx.clone(), tx) {
                Ok(imp) => Some(HotkeyRegistration { imp }),
                Err(e) => {
                    tracing::warn!("global hotkeys unavailable: {e}");
                    None
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = (ctx, tx);
            None
        }
    }

    /// Registers `hotkey`, replacing the previous one. The error is meant for the user.
    pub fn set(&mut self, hotkey: &Hotkey) -> Result<(), String> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            self.imp.set(hotkey)
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = hotkey;
            Ok(())
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use super::Hotkey;
    use crate::commands::{AppCommand, CommandSender};
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub struct Inner {
        manager: GlobalHotKeyManager,
        current: Option<HotKey>,
        registered_id: Arc<AtomicU32>,
    }

    impl Inner {
        pub fn new(ctx: eframe::egui::Context, tx: CommandSender) -> Result<Self, String> {
            let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
            let registered_id = Arc::new(AtomicU32::new(0));
            let id = registered_id.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                if event.state == HotKeyState::Pressed && event.id == id.load(Ordering::Relaxed) {
                    let _ = tx.send(AppCommand::ToggleBypass);
                    ctx.request_repaint();
                }
            }));
            Ok(Self {
                manager,
                current: None,
                registered_id,
            })
        }

        pub fn set(&mut self, hotkey: &Hotkey) -> Result<(), String> {
            let wanted = HotKey::from_str(&hotkey.to_global_string())
                .map_err(|e| format!("{hotkey} is not a valid hotkey: {e}"))?;
            if let Some(old) = self.current.take() {
                let _ = self.manager.unregister(old);
                self.registered_id.store(0, Ordering::Relaxed);
            }
            self.manager.register(wanted).map_err(|e| {
                format!("could not register {hotkey}: {e}. Another program may already use it; try a different combination")
            })?;
            self.registered_id.store(wanted.id(), Ordering::Relaxed);
            self.current = Some(wanted);
            Ok(())
        }
    }
}
