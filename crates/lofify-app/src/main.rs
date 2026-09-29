//! Lofify desktop application.
// Release builds on Windows are GUI programs: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    lofify_app::launch::run(&args)
}
