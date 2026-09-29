//! Logging to stderr and a daily rolling log file.

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Starts logging. Keep the returned guard alive for the life of the program so the file writer
/// is flushed on exit. Falls back to stderr only if the log directory cannot be created.
pub fn init(log_dir: Option<&Path>) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,naga=warn"));
    let file = log_dir.and_then(|dir| std::fs::create_dir_all(dir).ok().map(|()| dir));
    match file {
        Some(dir) => {
            let appender = tracing_appender::rolling::daily(dir, "lofify.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let result = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::io::stderr.and(writer))
                .try_init();
            if result.is_err() {
                return None;
            }
            Some(guard)
        }
        None => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .try_init();
            None
        }
    }
}
