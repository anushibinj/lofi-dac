//! Drawing the interface.

use crate::app::LofifyApp;
use crate::plot;
use crate::settings::CaptureKind;
use crate::wizard::{self, CableStatus, PlaybackStatus, Step};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use lofify_dsp::params::range;
use lofify_engine::engine::{EngineState, Status};
use std::ops::RangeInclusive;

const ACCENT: Color32 = Color32::from_rgb(38, 198, 178);
const OK: Color32 = Color32::from_rgb(90, 200, 120);
const WARN: Color32 = Color32::from_rgb(240, 180, 60);
const ERR: Color32 = Color32::from_rgb(235, 95, 95);
const MUTED: Color32 = Color32::from_rgb(150, 150, 160);

fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    r: &RangeInclusive<f32>,
    suffix: &str,
    log: bool,
) {
    ui.add(
        egui::Slider::new(value, r.clone())
            .text(label)
            .suffix(suffix)
            .logarithmic(log),
    );
}

fn percent_slider(ui: &mut egui::Ui, label: &str, value: &mut f32) {
    ui.add(
        egui::Slider::new(value, 0.0..=1.0)
            .text(label)
            .custom_formatter(|n, _| format!("{:.0} %", n * 100.0)),
    );
}

fn dot(ui: &mut egui::Ui, color: Color32, text: impl Into<String>) {
    ui.label(RichText::new(format!("\u{25CF} {}", text.into())).color(color));
}

/// Level meter: -60 dB to 0 dB, green / amber / red.
fn meter(ui: &mut egui::Ui, peak: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_gray(28));
    let db = 20.0 * peak.max(1.0e-6).log10();
    let to_x = |d: f32| rect.left() + rect.width() * ((d + 60.0) / 60.0).clamp(0.0, 1.0);
    let fill_to = to_x(db);
    for (from, to, color) in [(-60.0, -12.0, OK), (-12.0, -3.0, WARN), (-3.0, 0.0, ERR)] {
        let (x0, x1) = (to_x(from), to_x(to).min(fill_to));
        if x1 > x0 {
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
                0.0,
                color,
            );
        }
    }
    for tick in [-48.0, -36.0, -24.0, -12.0, -6.0] {
        let x = to_x(tick);
        painter.line_segment(
            [
                egui::pos2(x, rect.bottom() - 4.0),
                egui::pos2(x, rect.bottom()),
            ],
            egui::Stroke::new(1.0, Color32::from_gray(120)),
        );
    }
}

impl LofifyApp {
    /// Draws the whole window.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        if self.wizard.is_some() {
            self.ui_wizard(ui);
            return;
        }
        let status = self.engine().status();
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            self.ui_header(ui, &status);
            ui.add_space(6.0);
        });
        egui::Panel::bottom("statusbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.ui_statusbar(ui, &status);
            ui.add_space(3.0);
        });
        egui::Panel::left("controls")
            .resizable(false)
            .exact_size(340.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.ui_presets(ui);
                    ui.separator();
                    self.ui_devices(ui, &status);
                    ui.separator();
                    ui.label(RichText::new("Output level").strong());
                    meter(ui, status.metrics.output_peak);
                    ui.separator();
                    self.ui_plot(ui);
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.ui_sliders(ui);
                ui.add_space(8.0);
                self.ui_settings(ui);
            });
        });
    }

    fn ui_header(&mut self, ui: &mut egui::Ui, status: &Status) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Lofify").color(ACCENT).strong());
            ui.add_space(14.0);
            let power = egui::Button::new(
                RichText::new(if self.power {
                    "Power  ON"
                } else {
                    "Power  OFF"
                })
                .size(20.0)
                .strong()
                .color(if self.power {
                    Color32::BLACK
                } else {
                    Color32::WHITE
                }),
            )
            .fill(if self.power {
                ACCENT
            } else {
                Color32::from_gray(70)
            })
            .min_size(egui::vec2(150.0, 44.0));
            if ui
                .add(power)
                .on_hover_text("Start or stop processing all system audio")
                .clicked()
            {
                let on = !self.power;
                self.set_power(on);
            }
            let bypass = egui::Button::new(
                RichText::new(if self.bypass { "Bypass  ON" } else { "Bypass" })
                    .size(20.0)
                    .strong()
                    .color(if self.bypass {
                        Color32::BLACK
                    } else {
                        Color32::WHITE
                    }),
            )
            .fill(if self.bypass {
                WARN
            } else {
                Color32::from_gray(70)
            })
            .min_size(egui::vec2(150.0, 44.0));
            let hint = format!(
                "Play the original sound (20 ms crossfade). Hotkey: {}",
                self.settings.general.hotkey
            );
            if ui.add(bypass).on_hover_text(hint).clicked() {
                let now = !self.bypass;
                self.set_bypass(now);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &status.state {
                    EngineState::Running if status.metrics.waiting_for_audio => {
                        dot(ui, MUTED, "Running, waiting for audio")
                    }
                    EngineState::Running => dot(ui, OK, "Running"),
                    EngineState::Starting => dot(ui, WARN, "Starting"),
                    EngineState::Waiting(why) => dot(ui, WARN, format!("Waiting: {why}")),
                    EngineState::Error(why) => dot(ui, ERR, why.clone()),
                    EngineState::Stopped => dot(ui, MUTED, "Stopped"),
                };
            });
        });
    }

    fn ui_statusbar(&mut self, ui: &mut egui::Ui, status: &Status) {
        let m = &status.metrics;
        ui.horizontal_wrapped(|ui| {
            dot(
                ui,
                if m.audio_detected { OK } else { MUTED },
                if m.audio_detected {
                    "Audio detected"
                } else {
                    "No audio"
                },
            );
            ui.separator();
            let running = status.state == EngineState::Running;
            ui.label(format!(
                "Latency {}",
                if running {
                    format!("{:.0} ms", m.latency_ms)
                } else {
                    "-".into()
                }
            ));
            ui.separator();
            ui.label(format!(
                "Buffer {:.0} ms",
                if running { m.ring_fill_ms } else { 0.0 }
            ));
            ui.separator();
            let glitch = |n: u64| if n == 0 { MUTED } else { WARN };
            ui.label(
                RichText::new(format!("Underruns {}", m.underruns)).color(glitch(m.underruns)),
            );
            ui.label(RichText::new(format!("Overruns {}", m.overruns)).color(glitch(m.overruns)));
            if status.restarts > 0 {
                ui.label(RichText::new(format!("Reconnects {}", status.restarts)).color(WARN));
            }
            if let (Some(c), Some(o)) = (&status.capture, &status.output) {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} \u{2192} {} ({} Hz)",
                        c.device_name, o.device_name, o.sample_rate
                    ))
                    .color(MUTED),
                );
            }
            if let Some((msg, is_error)) = self.current_flash() {
                ui.separator();
                ui.label(RichText::new(msg).color(if is_error { ERR } else { OK }));
            }
        });
    }

    fn ui_presets(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Preset").strong());
        let modified = self.preset_modified();
        let shown = format!(
            "{}{}",
            self.settings.general.preset,
            if modified { " (modified)" } else { "" }
        );
        let builtin = lofify_dsp::presets::builtin_presets().len();
        let names: Vec<String> = self.presets.iter().map(|p| p.name.clone()).collect();
        let mut chosen: Option<String> = None;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("preset")
                .width(210.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if i == builtin {
                            ui.separator();
                            ui.label(RichText::new("Your presets").color(MUTED));
                        }
                        if ui
                            .selectable_label(*name == self.settings.general.preset, name)
                            .clicked()
                        {
                            chosen = Some(name.clone());
                        }
                    }
                });
            if ui.button("Save As...").clicked() {
                self.ui.save_open = !self.ui.save_open;
                self.ui.save_error = None;
            }
        });
        if let Some(name) = chosen {
            self.select_preset(&name);
        }
        if self.ui.save_open {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.ui.save_name)
                        .hint_text("Preset name")
                        .desired_width(170.0),
                );
                if ui.button("Save").clicked() {
                    let name = self.ui.save_name.clone();
                    match self.save_preset(&name) {
                        Ok(()) => {
                            self.ui.save_open = false;
                            self.ui.save_name.clear();
                            self.ui.save_error = None;
                        }
                        Err(e) => self.ui.save_error = Some(e),
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.ui.save_open = false;
                }
            });
            if let Some(e) = &self.ui.save_error {
                ui.label(RichText::new(e).color(ERR));
            }
        }
    }

    fn ui_devices(&mut self, ui: &mut egui::Ui, status: &Status) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Devices").strong());
            if ui.small_button("Refresh").clicked() {
                self.refresh_devices();
            }
        });
        if let Some(e) = &self.devices_error {
            ui.label(RichText::new(e).color(WARN));
        }

        // Capture source
        let audio = self.settings.audio.clone();
        let current = match audio.capture_kind {
            CaptureKind::Auto => "Auto-detect virtual cable".to_string(),
            CaptureKind::Input => format!("Input: {}", audio.capture_device),
            CaptureKind::Loopback if audio.capture_device.is_empty() => {
                "Loopback: default playback device".to_string()
            }
            CaptureKind::Loopback => format!("Loopback: {}", audio.capture_device),
        };
        let mut new_capture: Option<(CaptureKind, String)> = None;
        ui.label("Capture from");
        egui::ComboBox::from_id_salt("capture")
            .width(300.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        audio.capture_kind == CaptureKind::Auto,
                        "Auto-detect virtual cable",
                    )
                    .clicked()
                {
                    new_capture = Some((CaptureKind::Auto, String::new()));
                }
                for d in &self.devices.inputs {
                    let selected =
                        audio.capture_kind == CaptureKind::Input && audio.capture_device == d.name;
                    if ui
                        .selectable_label(selected, format!("Input: {}", d.name))
                        .clicked()
                    {
                        new_capture = Some((CaptureKind::Input, d.name.clone()));
                    }
                }
                for d in &self.devices.outputs {
                    let selected = audio.capture_kind == CaptureKind::Loopback
                        && audio.capture_device == d.name;
                    if ui
                        .selectable_label(selected, format!("Loopback: {}", d.name))
                        .clicked()
                    {
                        new_capture = Some((CaptureKind::Loopback, d.name.clone()));
                    }
                }
            });
        if let Some((kind, name)) = new_capture {
            self.settings.audio.capture_kind = kind;
            self.settings.audio.capture_device = name;
            self.apply_devices();
        }

        // Output device
        let shown = if audio.output_device.is_empty() {
            "System default".to_string()
        } else {
            audio.output_device.clone()
        };
        let mut new_output: Option<String> = None;
        ui.label("Play through");
        egui::ComboBox::from_id_salt("output")
            .width(300.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(audio.output_device.is_empty(), "System default")
                    .clicked()
                {
                    new_output = Some(String::new());
                }
                for d in &self.devices.outputs {
                    if d.is_virtual_cable() {
                        ui.add_enabled(
                            false,
                            egui::Label::new(format!("{} (virtual cable: not allowed)", d.name)),
                        );
                    } else if ui
                        .selectable_label(audio.output_device == d.name, &d.name)
                        .clicked()
                    {
                        new_output = Some(d.name.clone());
                    }
                }
            });
        if let Some(name) = new_output {
            self.settings.audio.output_device = name;
            self.apply_devices();
        }
        if audio.output_device.is_empty()
            && self
                .devices
                .default_output()
                .is_some_and(|d| d.is_virtual_cable())
        {
            ui.label(
                RichText::new("The system default output is the virtual cable. Choose your real speakers above.")
                    .color(WARN)
                    .small(),
            );
        }
        if let (Some(c), Some(o)) = (&status.capture, &status.output) {
            ui.label(
                RichText::new(format!(
                    "Capture {} Hz, {} ch \u{00B7} Output {} Hz, {} ch",
                    c.sample_rate, c.channels, o.sample_rate, o.channels
                ))
                .small()
                .color(MUTED),
            );
        }
    }

    fn ui_plot(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Filter response").strong());
        let points = plot::response_points(&self.params.filters, self.plot_rate);
        Plot::new("filter_response")
            .height(170.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .default_x_bounds(
                f64::from(plot::MIN_HZ.log10()),
                f64::from(plot::MAX_HZ.log10()),
            )
            .default_y_bounds(-48.0, 12.0)
            .x_axis_formatter(|mark, _| plot::axis_label(mark.value))
            .y_axis_formatter(|mark, _| format!("{:.0}", mark.value))
            .show(ui, |plot_ui| {
                plot_ui.line(
                    Line::new("Filters (dB)", PlotPoints::from(points))
                        .color(ACCENT)
                        .width(2.0),
                );
            });
    }

    fn ui_sliders(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.params;
        let open = |title: &str| {
            egui::CollapsingHeader::new(RichText::new(title).strong()).default_open(true)
        };
        open("1  Input").show(ui, |ui| {
            slider(
                ui,
                "Trim",
                &mut p.input.trim_db,
                &range::TRIM_DB,
                " dB",
                false,
            );
        });
        open("2  Tape wow and flutter").show(ui, |ui| {
            slider(
                ui,
                "Wow depth",
                &mut p.tape.wow_cents,
                &range::WOW_CENTS,
                " cents",
                false,
            );
            slider(
                ui,
                "Wow rate",
                &mut p.tape.wow_hz,
                &range::WOW_HZ,
                " Hz",
                false,
            );
            slider(
                ui,
                "Flutter depth",
                &mut p.tape.flutter_cents,
                &range::FLUTTER_CENTS,
                " cents",
                false,
            );
            slider(
                ui,
                "Flutter rate",
                &mut p.tape.flutter_hz,
                &range::FLUTTER_HZ,
                " Hz",
                false,
            );
            slider(
                ui,
                "Random drift",
                &mut p.tape.drift_cents,
                &range::DRIFT_CENTS,
                " cents",
                false,
            );
        });
        open("3  Saturation").show(ui, |ui| {
            slider(
                ui,
                "Drive",
                &mut p.saturation.drive,
                &range::DRIVE,
                "",
                false,
            );
            slider(
                ui,
                "Makeup gain",
                &mut p.saturation.makeup_db,
                &range::MAKEUP_DB,
                " dB",
                false,
            );
        });
        open("4  Bitcrusher").show(ui, |ui| {
            slider(
                ui,
                "Bit depth (24 = off)",
                &mut p.crush.bits,
                &range::BITS,
                " bits",
                false,
            );
            slider(
                ui,
                "Sample rate",
                &mut p.crush.sample_rate_hz,
                &range::CRUSH_RATE_HZ,
                " Hz",
                true,
            );
            ui.checkbox(&mut p.crush.dither, "Dither");
        });
        open("5  Filters").show(ui, |ui| {
            ui.checkbox(&mut p.filters.hpf_enabled, "High-pass");
            slider(
                ui,
                "High-pass cutoff",
                &mut p.filters.hpf_hz,
                &range::HPF_HZ,
                " Hz",
                true,
            );
            ui.checkbox(&mut p.filters.lpf_enabled, "Low-pass (the muffle)");
            slider(
                ui,
                "Low-pass cutoff",
                &mut p.filters.lpf_hz,
                &range::LPF_HZ,
                " Hz",
                true,
            );
            slider(
                ui,
                "Low-pass resonance (Q)",
                &mut p.filters.lpf_q,
                &range::LPF_Q,
                "",
                false,
            );
            slider(
                ui,
                "Warmth frequency",
                &mut p.filters.shelf_hz,
                &range::SHELF_HZ,
                " Hz",
                true,
            );
            slider(
                ui,
                "Warmth gain",
                &mut p.filters.shelf_db,
                &range::SHELF_DB,
                " dB",
                false,
            );
        });
        open("6  Reverb").show(ui, |ui| {
            percent_slider(ui, "Room size", &mut p.reverb.room);
            percent_slider(ui, "Damping", &mut p.reverb.damping);
            percent_slider(ui, "Mix", &mut p.reverb.mix);
        });
        open("7  Vinyl crackle and hiss").show(ui, |ui| {
            slider(
                ui,
                "Pops per second",
                &mut p.vinyl.pops_per_sec,
                &range::POPS_PER_SEC,
                "",
                false,
            );
            slider(
                ui,
                "Pop level",
                &mut p.vinyl.pop_db,
                &range::POP_DB,
                " dBFS",
                false,
            );
            slider(
                ui,
                "Hiss level (-90 = off)",
                &mut p.vinyl.hiss_db,
                &range::HISS_DB,
                " dBFS",
                false,
            );
        });
        open("8  Output").show(ui, |ui| {
            slider(
                ui,
                "Output gain",
                &mut p.output.gain_db,
                &range::OUTPUT_DB,
                " dB",
                false,
            );
            percent_slider(ui, "Wet / dry mix", &mut p.mix.wet_dry);
        });
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(RichText::new("Settings").strong()).show(ui, |ui| {
            let mut launch = self.settings.general.launch_at_login;
            if ui.checkbox(&mut launch, "Launch at login").changed() {
                self.set_launch_at_login(launch);
            }
            if ui
                .checkbox(
                    &mut self.settings.general.start_minimized,
                    "Start minimized to the tray",
                )
                .changed()
            {
                self.mark_dirty();
            }
            if ui
                .checkbox(
                    &mut self.settings.general.auto_start,
                    "Start processing when the app opens",
                )
                .changed()
            {
                self.mark_dirty();
            }
            if ui
                .checkbox(
                    &mut self.settings.audio.low_latency,
                    "Low-latency mode (applies after restart; needs stable devices)",
                )
                .changed()
            {
                self.mark_dirty();
            }
            ui.horizontal(|ui| {
                ui.label("Bypass hotkey");
                ui.add(egui::TextEdit::singleline(&mut self.ui.hotkey_text).desired_width(140.0));
                if ui.button("Apply").clicked() {
                    self.apply_hotkey_setting();
                }
            });
            if ui.button("Run the setup wizard again").clicked() {
                self.restart_wizard();
            }
            if let Some(paths) = &self.paths {
                ui.label(
                    RichText::new(format!("Settings: {}", paths.settings_file.display()))
                        .small()
                        .color(MUTED),
                );
                ui.label(
                    RichText::new(format!("Logs: {}", paths.log_dir.display()))
                        .small()
                        .color(MUTED),
                );
            }
        });
    }

    // ---- first-run wizard ------------------------------------------------------------------

    fn ui_wizard(&mut self, ui: &mut egui::Ui) {
        let Some(state) = &self.wizard else { return };
        let (step, mut output) = (state.step, state.output.clone());
        let status = self.engine().status();
        let mut go_next = false;
        let mut go_back = false;
        let mut skip = false;
        let mut finish = false;
        let can_next =
            wizard::can_advance(step, &self.devices, &output, status.metrics.audio_detected);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.add_space(18.0);
                ui.heading(RichText::new("Welcome to Lofify").color(ACCENT).strong());
                ui.label(RichText::new(format!("Step {} of {}: {}", step.number(), Step::ALL.len(), step.title())).strong());
                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.set_width(520.0);
                    match step {
                        Step::Cable => match wizard::cable_status(&self.devices) {
                            CableStatus::Found(name) => {
                                ui.label(RichText::new(format!("\u{2714} Found the virtual cable: {name}")).color(OK));
                            }
                            CableStatus::Missing => {
                                ui.label(RichText::new("\u{2718} No virtual audio cable found").color(WARN));
                                ui.label(wizard::INSTALL_INSTRUCTIONS);
                                ui.hyperlink_to("Open vb-audio.com/Cable", wizard::VB_CABLE_URL);
                            }
                        },
                        Step::DefaultPlayback => {
                            match wizard::playback_status(&self.devices) {
                                PlaybackStatus::CableIsDefault => {
                                    ui.label(RichText::new("\u{2714} Windows is playing sound through the cable").color(OK));
                                }
                                PlaybackStatus::OtherDefault(name) => {
                                    ui.label(RichText::new(format!("\u{2718} Windows currently plays through: {name}")).color(WARN));
                                }
                                PlaybackStatus::NoDevice => {
                                    ui.label(RichText::new("\u{2718} No playback device found").color(ERR));
                                }
                            }
                            ui.label(wizard::DEFAULT_PLAYBACK_INSTRUCTIONS);
                        }
                        Step::Output => {
                            ui.label("Pick the speakers or headphones you actually listen on. Never the cable.");
                            let real: Vec<String> = wizard::real_outputs(&self.devices).iter().map(|d| d.name.clone()).collect();
                            egui::ComboBox::from_id_salt("wizard_output")
                                .width(440.0)
                                .selected_text(if output.is_empty() { "Choose a device".to_string() } else { output.clone() })
                                .show_ui(ui, |ui| {
                                    for name in &real {
                                        if ui.selectable_label(output == *name, name).clicked() {
                                            output = name.clone();
                                        }
                                    }
                                });
                        }
                        Step::Confirm => {
                            ui.label("Lofify is running. Play some music or a video now.");
                            let m = &status.metrics;
                            if m.audio_detected {
                                ui.label(RichText::new("\u{2714} Audio detected. It works!").color(OK).strong());
                                ui.label(format!("Latency about {:.0} ms", m.latency_ms));
                            } else {
                                match &status.state {
                                    EngineState::Running => ui.label(RichText::new("Waiting to hear audio...").color(WARN)),
                                    EngineState::Error(e) | EngineState::Waiting(e) => ui.label(RichText::new(e).color(ERR)),
                                    _ => ui.label(RichText::new("Starting...").color(MUTED)),
                                };
                            }
                        }
                    }
                    if matches!(step, Step::Cable | Step::DefaultPlayback) && ui.button("Re-check").clicked() {
                        self.refresh_devices();
                    }
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if step.previous().is_some() && ui.button("Back").clicked() {
                        go_back = true;
                    }
                    if step == Step::Confirm {
                        if ui.add_enabled(can_next, egui::Button::new("Finish")).clicked() {
                            finish = true;
                        }
                        if !can_next && ui.button("Finish anyway").clicked() {
                            finish = true;
                        }
                    } else if ui.add_enabled(can_next, egui::Button::new("Next")).clicked() {
                        go_next = true;
                    }
                    if ui.button("Skip setup").clicked() {
                        skip = true;
                    }
                });
            });
        });

        if let Some(w) = &mut self.wizard {
            w.output = output.clone();
        }
        if go_back && let (Some(w), Some(prev)) = (&mut self.wizard, step.previous()) {
            w.step = prev;
        }
        if go_next {
            if step == Step::Output {
                // Entering the last step: apply the choices and start processing.
                self.settings.audio.capture_kind = CaptureKind::Auto;
                self.settings.audio.capture_device.clear();
                self.settings.audio.output_device = output.clone();
                self.apply_devices();
                self.set_power(true);
            }
            if let (Some(w), Some(next)) = (&mut self.wizard, step.next()) {
                w.step = next;
            }
        }
        if finish {
            self.finish_wizard(&output);
        }
        if skip {
            self.settings.general.wizard_done = true;
            self.wizard = None;
            self.mark_dirty();
        }
    }
}

impl eframe::App for LofifyApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }
}
