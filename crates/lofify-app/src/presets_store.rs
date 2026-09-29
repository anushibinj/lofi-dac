//! User presets: TOML files in the config directory, listed next to the built-in ones.

use lofify_dsp::presets::{Preset, builtin_presets};
use std::io;
use std::path::{Path, PathBuf};

/// Why a preset name cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    /// The name belongs to a built-in preset.
    Reserved(String),
    /// The name has no usable characters for a file name.
    Unusable,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::Empty => write!(f, "enter a name for the preset"),
            NameError::TooLong => write!(f, "the name is too long (60 characters at most)"),
            NameError::Reserved(n) => {
                write!(f, "\"{n}\" is a built-in preset; choose another name")
            }
            NameError::Unusable => write!(f, "the name needs at least one letter or digit"),
        }
    }
}

/// A file-name friendly version of a preset name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Checks that `name` can be saved as a user preset.
pub fn validate_name(name: &str) -> Result<String, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > 60 {
        return Err(NameError::TooLong);
    }
    if slug(name).is_empty() {
        return Err(NameError::Unusable);
    }
    if let Some(b) = builtin_presets()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
    {
        return Err(NameError::Reserved(b.name));
    }
    Ok(name.to_string())
}

/// Saves a user preset as `<dir>/<slug>.toml`, replacing an earlier preset with the same name.
pub fn save(dir: &Path, preset: &Preset) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.toml", slug(&preset.name)));
    let text = preset
        .to_toml()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Loads every valid `*.toml` preset in `dir`, sorted by name. Unreadable files are skipped and
/// reported in the second element.
pub fn load_all(dir: &Path) -> (Vec<Preset>, Vec<String>) {
    let mut presets = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (presets, problems);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| Preset::from_toml(&t).map_err(|e| e.to_string()))
        {
            Ok(p) if !p.name.trim().is_empty() => presets.push(p),
            Ok(_) => problems.push(format!("{} has no name", path.display())),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    presets.sort_by_key(|p| p.name.to_lowercase());
    (presets, problems)
}

/// Deletes a user preset file. Returns whether a file was removed.
pub fn delete(dir: &Path, name: &str) -> io::Result<bool> {
    let path = dir.join(format!("{}.toml", slug(name)));
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofify_dsp::params::Params;

    #[test]
    fn slugs() {
        assert_eq!(slug("My Cool Preset!"), "my-cool-preset");
        assert_eq!(slug("  ---  "), "");
        assert_eq!(slug("Été 2"), "été-2");
    }

    #[test]
    fn name_validation() {
        assert_eq!(validate_name("  Sleepy  ").unwrap(), "Sleepy");
        assert_eq!(validate_name(""), Err(NameError::Empty));
        assert_eq!(validate_name("   "), Err(NameError::Empty));
        assert_eq!(validate_name(&"x".repeat(61)), Err(NameError::TooLong));
        assert_eq!(validate_name("!!!"), Err(NameError::Unusable));
        assert_eq!(
            validate_name("classic lofi"),
            Err(NameError::Reserved("Classic Lofi".into()))
        );
        assert!(
            validate_name("CLEAN")
                .unwrap_err()
                .to_string()
                .contains("built-in")
        );
    }

    #[test]
    fn save_load_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut params = Params::default();
        params.reverb.mix = 0.33;
        let preset = Preset::new("Sleepy", params);
        let path = save(dir.path(), &preset).unwrap();
        assert_eq!(path.file_name().unwrap(), "sleepy.toml");

        let (loaded, problems) = load_all(dir.path());
        assert!(problems.is_empty());
        assert_eq!(loaded, vec![preset.clone()]);

        assert!(delete(dir.path(), "Sleepy").unwrap());
        assert!(!delete(dir.path(), "Sleepy").unwrap());
        assert!(load_all(dir.path()).0.is_empty());
    }

    #[test]
    fn saving_again_replaces_the_preset() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Params::default();
        p.reverb.mix = 0.1;
        save(dir.path(), &Preset::new("Same", p)).unwrap();
        p.reverb.mix = 0.9;
        save(dir.path(), &Preset::new("Same", p)).unwrap();
        let (loaded, _) = load_all(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].params.reverb.mix, 0.9);
    }

    #[test]
    fn bad_files_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("broken.toml"), "nope = = =").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "ignored").unwrap();
        save(dir.path(), &Preset::new("Good", Params::default())).unwrap();
        let (loaded, problems) = load_all(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("broken.toml"));
    }

    #[test]
    fn missing_directory_is_empty() {
        let (p, e) = load_all(Path::new("/definitely/not/here"));
        assert!(p.is_empty() && e.is_empty());
    }

    #[test]
    fn presets_are_sorted_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["banana", "Apple", "cherry"] {
            save(dir.path(), &Preset::new(n, Params::default())).unwrap();
        }
        let names: Vec<String> = load_all(dir.path()).0.into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Apple", "banana", "cherry"]);
    }
}
