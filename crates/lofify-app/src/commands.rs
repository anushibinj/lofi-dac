//! Commands that can arrive from outside the UI thread (tray menu, global hotkey).

/// Something the tray icon or hotkey asked the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    /// Bring the window back from the tray.
    ShowWindow,
    /// Toggle the bypass crossfade.
    ToggleBypass,
    /// Quit the application (the window's close button only hides it).
    Quit,
}

/// Sender side, cloned into OS event handlers.
pub type CommandSender = crossbeam_channel::Sender<AppCommand>;
/// Receiver side, drained by the app every frame.
pub type CommandReceiver = crossbeam_channel::Receiver<AppCommand>;

/// Creates the command channel.
pub fn channel() -> (CommandSender, CommandReceiver) {
    crossbeam_channel::unbounded()
}
