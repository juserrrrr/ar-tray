//! A small text log next to the app data, so a failure leaves a trace to read.

use std::fmt::Display;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 256 * 1024;

static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Starts the log in `dir` and records panics with their location and backtrace.
pub fn init(dir: &Path) {
    let file = dir.join("log.txt");
    // Keeps one older file instead of growing forever.
    if fs::metadata(&file).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = fs::rename(&file, dir.join("log.old.txt"));
    }
    let _ = FILE.set(file);
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        line(format_args!("falha: {info}\n{backtrace}"));
    }));
    line(format_args!("Painel do Ar {} iniciado", env!("CARGO_PKG_VERSION")));
}

pub fn line(message: impl Display) {
    let Some(file) = FILE.get() else { return };
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    if let Ok(mut out) = OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(out, "[{seconds}] {message}");
    }
}
