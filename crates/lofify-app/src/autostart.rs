//! "Launch at login" through the OS: the registry Run key on Windows, a launch agent on macOS.

/// Whether the app is registered to start at login.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Registers or unregisters the app. Returns a message suitable for showing the user on failure.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    imp::set_enabled(enabled)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use auto_launch::AutoLaunchBuilder;

    fn build() -> Result<auto_launch::AutoLaunch, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find the app path: {e}"))?;
        let path = exe
            .to_str()
            .ok_or_else(|| "the app path is not valid UTF-8".to_string())?;
        AutoLaunchBuilder::new()
            .set_app_name("Lofify")
            .set_app_path(path)
            .set_args(&["--minimized"])
            .build()
            .map_err(|e| e.to_string())
    }

    pub fn is_enabled() -> bool {
        build()
            .and_then(|a| a.is_enabled().map_err(|e| e.to_string()))
            .unwrap_or(false)
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let launcher = build()?;
        let result = if enabled {
            launcher.enable()
        } else {
            launcher.disable()
        };
        result.map_err(|e| format!("could not update launch at login: {e}"))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_enabled: bool) -> Result<(), String> {
        Err("launch at login is only supported on Windows and macOS".to_string())
    }
}
