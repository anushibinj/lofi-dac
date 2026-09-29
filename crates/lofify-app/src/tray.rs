//! System tray icon with a Show / Toggle bypass / Quit menu.
//!
//! Real implementation on Windows and macOS (`tray-icon`); on other systems a no-op stub so the
//! rest of the app does not care. **UNTESTED on real desktops** (compile-checked only).

use crate::commands::{AppCommand, CommandSender};

/// Handle to the tray icon. Dropping it removes the icon.
pub struct Tray {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    imp: imp::Inner,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").finish_non_exhaustive()
    }
}

impl Tray {
    /// Creates the icon. Must be called on the main thread after the event loop has started
    /// (inside eframe's creation callback). Returns `None` if the platform refuses.
    pub fn create(ctx: &eframe::egui::Context, tx: CommandSender) -> Option<Tray> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            match imp::Inner::new(ctx.clone(), tx) {
                Ok(imp) => Some(Tray { imp }),
                Err(e) => {
                    tracing::warn!("tray icon unavailable: {e}");
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

    /// Reflects the bypass state in the menu check mark and the icon.
    pub fn set_bypass(&self, bypassed: bool) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.imp.set_bypass(bypassed);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = bypassed;
    }
}

/// Maps a menu item name to the command it triggers (kept separate so it is testable).
pub fn command_for(label: &str) -> Option<AppCommand> {
    match label {
        "Show" => Some(AppCommand::ShowWindow),
        "Toggle bypass" => Some(AppCommand::ToggleBypass),
        "Quit" => Some(AppCommand::Quit),
        _ => None,
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use super::{AppCommand, CommandSender, command_for};
    use crate::icon;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    pub struct Inner {
        tray: TrayIcon,
        bypass_item: CheckMenuItem,
    }

    fn icon(active: bool) -> Result<Icon, String> {
        Icon::from_rgba(icon::rgba(32, active), 32, 32).map_err(|e| e.to_string())
    }

    impl Inner {
        pub fn new(ctx: eframe::egui::Context, tx: CommandSender) -> Result<Self, String> {
            let show = MenuItem::new("Show", true, None);
            let bypass_item = CheckMenuItem::new("Toggle bypass", true, false, None);
            let quit = MenuItem::new("Quit", true, None);
            let menu = Menu::new();
            menu.append_items(&[&show, &bypass_item, &PredefinedMenuItem::separator(), &quit])
                .map_err(|e| e.to_string())?;

            let ids: [(MenuId, &'static str); 3] = [
                (show.id().clone(), "Show"),
                (bypass_item.id().clone(), "Toggle bypass"),
                (quit.id().clone(), "Quit"),
            ];
            {
                let (tx, ctx) = (tx.clone(), ctx.clone());
                MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    let label = ids.iter().find(|(id, _)| *id == event.id).map(|(_, l)| *l);
                    if let Some(cmd) = label.and_then(command_for) {
                        let _ = tx.send(cmd);
                        // `App::logic` also runs while the window is hidden after a repaint request.
                        ctx.request_repaint();
                    }
                }));
            }
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = tx.send(AppCommand::ShowWindow);
                    ctx.request_repaint();
                }
            }));

            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("Lofify")
                .with_icon(icon(true)?)
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Self { tray, bypass_item })
        }

        pub fn set_bypass(&self, bypassed: bool) {
            self.bypass_item.set_checked(bypassed);
            let _ = self.tray.set_icon(icon(!bypassed).ok());
            let _ = self.tray.set_tooltip(Some(if bypassed {
                "Lofify (bypassed)"
            } else {
                "Lofify"
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_labels_map_to_commands() {
        assert_eq!(command_for("Show"), Some(AppCommand::ShowWindow));
        assert_eq!(command_for("Toggle bypass"), Some(AppCommand::ToggleBypass));
        assert_eq!(command_for("Quit"), Some(AppCommand::Quit));
        assert_eq!(command_for("Nope"), None);
    }
}
