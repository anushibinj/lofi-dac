//! Application state and behaviour. Drawing lives in [`crate::ui`].

use crate::autostart;
use crate::commands::{AppCommand, CommandReceiver};
use crate::hotkey;
use crate::hotkeys::HotkeyRegistration;
use crate::presets_store;
use crate::settings::{AppPaths, Settings};
use crate::tray::Tray;
use crate::wizard;
use eframe::egui;
use lofify_dsp::params::Params;
use lofify_dsp::presets::{Preset, builtin_presets};
use lofify_engine::engine::Engine;
use lofify_engine::error::Result as EngineResult;
use lofify_engine::types::DeviceList;
use std::time::{Duration, Instant};

/// How often the device pickers refresh.
const DEVICE_REFRESH: Duration = Duration::from_millis(2000);
/// Settings are written this long after the last change.
const SAVE_DELAY: Duration = Duration::from_millis(1000);
/// How long a status message stays visible.
const FLASH_FOR: Duration = Duration::from_secs(6);

/// OS integrations the app owns. Tests use [`Services::none`].
#[derive(Debug, Default)]
pub struct Services {
    pub tray: Option<Tray>,
    pub hotkeys: Option<HotkeyRegistration>,
}

impl Services {
    pub fn none() -> Self {
        Self::default()
    }
}

/// Transient UI state.
#[derive(Debug, Default)]
pub struct UiState {
    pub save_open: bool,
    pub save_name: String,
    pub save_error: Option<String>,
    pub hotkey_text: String,
    pub settings_open: bool,
}

/// State of the first-run wizard while it is showing.
#[derive(Debug)]
pub struct WizardState {
    pub step: wizard::Step,
    pub output: String,
}

/// Source of the device list, injectable for tests.
pub type DeviceSource = Box<dyn Fn() -> EngineResult<DeviceList>>;

/// The whole application.
pub struct LofifyApp {
    pub engine: Engine,
    pub settings: Settings,
    pub paths: Option<AppPaths>,
    /// The parameters shown in the sliders and sent to the engine.
    pub params: Params,
    last_sent: Params,
    pub bypass: bool,
    /// The user's Power switch.
    pub power: bool,
    /// Built-in presets first, then the user's.
    pub presets: Vec<Preset>,
    pub devices: DeviceList,
    pub devices_error: Option<String>,
    device_source: DeviceSource,
    last_device_refresh: Option<Instant>,
    services: Services,
    commands: CommandReceiver,
    hidden: bool,
    quit_requested: bool,
    pub ui: UiState,
    pub wizard: Option<WizardState>,
    dirty_since: Option<Instant>,
    flash: Option<(String, Instant, bool)>,
    /// Sample rate used for the frequency-response plot.
    pub plot_rate: f32,
}

impl std::fmt::Debug for LofifyApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LofifyApp")
            .field("power", &self.power)
            .field("bypass", &self.bypass)
            .finish_non_exhaustive()
    }
}

impl LofifyApp {
    /// Builds the app. `warning` (from loading settings or presets) is shown to the user.
    pub fn new(
        engine: Engine,
        settings: Settings,
        paths: Option<AppPaths>,
        services: Services,
        commands: CommandReceiver,
        device_source: DeviceSource,
        warning: Option<String>,
    ) -> Self {
        let mut app = Self {
            engine,
            params: settings.params,
            last_sent: settings.params,
            bypass: false,
            power: false,
            presets: Vec::new(),
            devices: DeviceList::default(),
            devices_error: None,
            device_source,
            last_device_refresh: None,
            services,
            commands,
            hidden: false,
            quit_requested: false,
            ui: UiState {
                hotkey_text: settings.general.hotkey.clone(),
                ..UiState::default()
            },
            wizard: None,
            dirty_since: None,
            flash: None,
            plot_rate: 48_000.0,
            settings,
            paths,
        };
        app.reload_presets();
        app.engine.params().store(&app.params);
        app.apply_hotkey_setting();
        if let Some(w) = warning {
            app.flash(w, true);
        }
        app.refresh_devices();
        // The wizard only exists for the Windows virtual-cable flow.
        if cfg!(target_os = "windows") && !app.settings.general.wizard_done {
            let output = wizard::suggested_output(&app.devices, &app.settings.audio.output_device)
                .unwrap_or_default();
            app.wizard = Some(WizardState {
                step: wizard::Step::Cable,
                output,
            });
        } else if !app.settings.general.wizard_done {
            app.settings.general.wizard_done = true;
        }
        if app.wizard.is_none() && app.settings.general.auto_start {
            app.set_power(true);
        }
        app
    }

    /// Whether the window should start hidden in the tray.
    pub fn starts_hidden(&self, force_minimized: bool) -> bool {
        (force_minimized || self.settings.general.start_minimized)
            && self.wizard.is_none()
            && self.services.tray.is_some()
    }

    // ---- messages --------------------------------------------------------------------------

    pub fn flash(&mut self, message: impl Into<String>, is_error: bool) {
        let message = message.into();
        if is_error {
            tracing::warn!("{message}");
        }
        self.flash = Some((message, Instant::now(), is_error));
    }

    /// The current transient message, if it has not expired.
    pub fn current_flash(&self) -> Option<(&str, bool)> {
        self.flash
            .as_ref()
            .filter(|(_, at, _)| at.elapsed() < FLASH_FOR)
            .map(|(m, _, e)| (m.as_str(), *e))
    }

    // ---- engine control --------------------------------------------------------------------

    pub fn set_power(&mut self, on: bool) {
        self.power = on;
        if on {
            self.engine.set_devices(self.settings.audio.device_config());
            self.engine.start();
        } else {
            self.engine.stop();
        }
    }

    /// Re-applies the device choice (after the user changed a picker).
    pub fn apply_devices(&mut self) {
        self.engine.set_devices(self.settings.audio.device_config());
        self.mark_dirty();
    }

    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
        self.engine.params().set_bypass(bypass);
        if let Some(t) = &self.services.tray {
            t.set_bypass(bypass);
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Pushes changed parameters to the audio thread.
    fn sync_engine(&mut self) {
        if self.params != self.last_sent {
            self.engine.params().store(&self.params);
            self.last_sent = self.params;
            self.settings.params = self.params;
            self.mark_dirty();
        }
        self.engine.params().set_bypass(self.bypass);
    }

    // ---- presets ---------------------------------------------------------------------------

    pub fn reload_presets(&mut self) {
        let mut list = builtin_presets();
        if let Some(paths) = &self.paths {
            let (user, problems) = presets_store::load_all(&paths.presets_dir);
            list.extend(user);
            if let Some(first) = problems.first() {
                tracing::warn!("some presets could not be loaded: {first}");
            }
        }
        self.presets = list;
    }

    /// The preset with the given name, if any.
    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == name)
    }

    pub fn select_preset(&mut self, name: &str) {
        if let Some((params, name)) = self.preset(name).map(|p| (p.params, p.name.clone())) {
            self.params = params;
            self.settings.general.preset = name;
            self.mark_dirty();
        }
    }

    /// True when the sliders no longer match the selected preset.
    pub fn preset_modified(&self) -> bool {
        self.preset(&self.settings.general.preset)
            .is_none_or(|p| p.params != self.params)
    }

    /// Saves the current parameters as a user preset called `name`.
    pub fn save_preset(&mut self, name: &str) -> Result<(), String> {
        let name = presets_store::validate_name(name).map_err(|e| e.to_string())?;
        let Some(paths) = &self.paths else {
            return Err("no config folder is available on this system".into());
        };
        let mut preset = Preset::new(name.clone(), self.params);
        preset.description = "Saved from the Lofify app.".into();
        presets_store::save(&paths.presets_dir, &preset)
            .map_err(|e| format!("could not save the preset: {e}"))?;
        self.reload_presets();
        self.settings.general.preset = name.clone();
        self.mark_dirty();
        self.flash(format!("Saved preset \"{name}\""), false);
        Ok(())
    }

    // ---- devices ---------------------------------------------------------------------------

    pub fn refresh_devices(&mut self) {
        self.last_device_refresh = Some(Instant::now());
        match (self.device_source)() {
            Ok(list) => {
                self.devices = list;
                self.devices_error = None;
            }
            Err(e) => self.devices_error = Some(e.to_string()),
        }
    }

    fn maybe_refresh_devices(&mut self) {
        if self
            .last_device_refresh
            .is_none_or(|t| t.elapsed() >= DEVICE_REFRESH)
        {
            self.refresh_devices();
        }
    }

    // ---- settings --------------------------------------------------------------------------

    pub fn mark_dirty(&mut self) {
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    pub fn save_settings_now(&mut self) {
        self.dirty_since = None;
        // The sliders may have changed since the last frame synced them.
        self.settings.params = self.params;
        let Some(paths) = &self.paths else { return };
        if let Err(e) = self.settings.save(&paths.settings_file) {
            tracing::warn!("could not save settings: {e}");
        }
    }

    fn maybe_save_settings(&mut self) {
        if self.dirty_since.is_some_and(|t| t.elapsed() >= SAVE_DELAY) {
            self.save_settings_now();
        }
    }

    /// Applies the hotkey text from the settings; reports problems to the user.
    pub fn apply_hotkey_setting(&mut self) {
        match hotkey::parse(&self.ui.hotkey_text) {
            Ok(h) => {
                let result = self.services.hotkeys.as_mut().map_or(Ok(()), |r| r.set(&h));
                match result {
                    Ok(()) => {
                        self.settings.general.hotkey = h.to_string();
                        self.ui.hotkey_text = h.to_string();
                        self.mark_dirty();
                    }
                    Err(e) => self.flash(e, true),
                }
            }
            Err(e) => self.flash(format!("Hotkey: {e}"), true),
        }
    }

    pub fn set_launch_at_login(&mut self, enabled: bool) {
        match autostart::set_enabled(enabled) {
            Ok(()) => {
                self.settings.general.launch_at_login = enabled;
                self.mark_dirty();
            }
            Err(e) => {
                self.settings.general.launch_at_login = autostart::is_enabled();
                self.flash(e, true);
            }
        }
    }

    pub fn finish_wizard(&mut self, output: &str) {
        self.settings.audio.output_device = output.to_string();
        self.settings.general.wizard_done = true;
        self.wizard = None;
        self.mark_dirty();
        self.apply_devices();
        if !self.power {
            self.set_power(true);
        }
    }

    pub fn restart_wizard(&mut self) {
        let output = wizard::suggested_output(&self.devices, &self.settings.audio.output_device)
            .unwrap_or_default();
        self.wizard = Some(WizardState {
            step: wizard::Step::Cable,
            output,
        });
    }

    // ---- per-frame logic -------------------------------------------------------------------

    /// Runs before each frame, and also while the window is hidden after a repaint request
    /// (which is how the tray menu and the hotkey reach us).
    pub fn tick(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                AppCommand::ShowWindow => self.show_window(ctx),
                AppCommand::ToggleBypass => {
                    let now = !self.bypass;
                    self.set_bypass(now);
                    self.flash(if now { "Bypass on" } else { "Bypass off" }, false);
                }
                AppCommand::Quit => {
                    self.quit_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Closing the window hides it to the tray instead of quitting, when a tray exists.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quit_requested
            && self.services.tray.is_some()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
            self.save_settings_now();
        }

        self.sync_engine();
        self.maybe_save_settings();
        if !self.hidden {
            self.maybe_refresh_devices();
            // Meters and the status line animate, so keep repainting at ~30 fps while visible.
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// The stored bypass hotkey in canonical form.
    pub fn settings_hotkey(&self) -> &str {
        &self.settings.general.hotkey
    }

    /// Sets the hotkey text field (the user typing); call `apply_hotkey_setting` to commit.
    pub fn set_hotkey_text(&mut self, text: &str) {
        self.ui.hotkey_text = text.to_string();
    }

    /// The wizard's current step, if the wizard is showing.
    pub fn wizard_step(&self) -> Option<wizard::Step> {
        self.wizard.as_ref().map(|w| w.step)
    }

    /// Hides the window to the tray (used by the start-minimized option).
    pub fn hide(&mut self) {
        self.hidden = true;
    }

    /// Called when the application exits.
    pub fn shutdown(&mut self) {
        self.save_settings_now();
        self.engine.stop();
    }
}
