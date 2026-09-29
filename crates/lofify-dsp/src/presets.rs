//! Presets: a named [`Params`] set stored as TOML.
//!
//! The five built-in presets are TOML files embedded at compile time (`presets/*.toml`), so they
//! double as documentation of the file format users can write themselves.

use crate::params::Params;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A named parameter set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub description: String,
    #[serde(flatten)]
    pub params: Params,
}

/// Error returned when a preset cannot be parsed or serialised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetError(String);

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "preset error: {}", self.0)
    }
}

impl std::error::Error for PresetError {}

/// Name of the preset used when nothing else is selected.
pub const DEFAULT_PRESET: &str = "Classic Lofi";

const BUILTIN_SOURCES: [&str; 5] = [
    include_str!("../presets/classic-lofi.toml"),
    include_str!("../presets/late-night.toml"),
    include_str!("../presets/vinyl-dust.toml"),
    include_str!("../presets/cassette.toml"),
    include_str!("../presets/clean.toml"),
];

impl Preset {
    /// Creates a preset from a name and parameters.
    pub fn new(name: impl Into<String>, params: Params) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            params,
        }
    }

    /// Parses a preset from TOML. Missing fields take their "off" defaults and out-of-range
    /// values are clamped.
    pub fn from_toml(text: &str) -> Result<Preset, PresetError> {
        let mut preset: Preset = toml::from_str(text).map_err(|e| PresetError(e.to_string()))?;
        preset.params = preset.params.sanitized();
        Ok(preset)
    }

    /// Serialises the preset to pretty TOML.
    pub fn to_toml(&self) -> Result<String, PresetError> {
        toml::to_string_pretty(self).map_err(|e| PresetError(e.to_string()))
    }
}

/// The five built-in presets in menu order.
pub fn builtin_presets() -> Vec<Preset> {
    BUILTIN_SOURCES
        .iter()
        .filter_map(|src| Preset::from_toml(src).ok())
        .collect()
}

/// Looks up a built-in preset by name (case-insensitive).
pub fn builtin_preset(name: &str) -> Option<Preset> {
    builtin_presets()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_presets_parse() {
        for src in BUILTIN_SOURCES {
            let p = Preset::from_toml(src).expect("built-in preset must parse");
            assert!(!p.name.is_empty());
            assert!(!p.description.is_empty());
        }
        assert_eq!(builtin_presets().len(), BUILTIN_SOURCES.len());
    }

    #[test]
    fn ships_the_five_named_presets_in_order() {
        let names: Vec<String> = builtin_presets().into_iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "Classic Lofi",
                "Late Night",
                "Vinyl Dust",
                "Cassette",
                "Clean"
            ]
        );
    }

    #[test]
    fn builtin_values_are_already_in_range() {
        for p in builtin_presets() {
            assert_eq!(
                p.params,
                p.params.sanitized(),
                "{} has out-of-range values",
                p.name
            );
        }
    }

    #[test]
    fn classic_lofi_matches_the_brief() {
        let p = builtin_preset("classic lofi")
            .expect("preset exists")
            .params;
        assert_eq!(p.filters.hpf_hz, 90.0);
        assert!(p.filters.hpf_enabled);
        assert_eq!(p.filters.lpf_hz, 4_500.0);
        assert_eq!(p.filters.lpf_q, 0.7);
        assert!(p.filters.lpf_enabled);
        assert_eq!(p.filters.shelf_db, 2.0);
        assert_eq!(p.filters.shelf_hz, 200.0);
        assert_eq!(p.crush.bits, 12.0);
        assert_eq!(p.crush.sample_rate_hz, 20_000.0);
        assert_eq!(p.saturation.drive, 1.5);
        assert_eq!(p.vinyl.pops_per_sec, 6.0);
        assert_eq!(p.vinyl.pop_db, -34.0);
        assert_eq!(p.vinyl.hiss_db, -50.0);
        assert_eq!(p.reverb.mix, 0.12);
        assert_eq!(p.mix.wet_dry, 1.0);
        assert_eq!(p.tape.wow_cents, 8.0);
        assert_eq!(p.tape.wow_hz, 0.5);
        assert_eq!(p.tape.flutter_cents, 2.0);
        assert_eq!(p.tape.flutter_hz, 6.5);
    }

    #[test]
    fn clean_is_all_effects_off() {
        let p = builtin_preset("Clean").expect("preset exists");
        assert_eq!(p.params, Params::default());
    }

    #[test]
    fn toml_roundtrip_preserves_everything() {
        for p in builtin_presets() {
            let text = p.to_toml().expect("serialises");
            let back = Preset::from_toml(&text).expect("parses again");
            assert_eq!(p, back);
        }
    }

    #[test]
    fn user_files_may_be_partial_and_out_of_range() {
        let p =
            Preset::from_toml("name = \"Mine\"\n[reverb]\nmix = 7.0\n[tape]\nwow_cents = 3.0\n")
                .expect("parses");
        assert_eq!(p.name, "Mine");
        assert_eq!(p.params.reverb.mix, 1.0);
        assert_eq!(p.params.tape.wow_cents, 3.0);
        assert_eq!(p.params.filters, Params::default().filters);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(Preset::from_toml("this is not = = toml [").is_err());
        assert!(Preset::from_toml("[tape]\nwow_cents = \"loud\"\n").is_err());
    }

    #[test]
    fn unknown_lookup_is_none() {
        assert!(builtin_preset("Nope").is_none());
    }
}
