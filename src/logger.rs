//! Persistent file log — `app.log` next to the executable.
//!
//! Every scheduler event is appended here so problems can be diagnosed
//! even when the UI is hidden in the tray. The file self-trims: once it
//! grows past `MAX_BYTES`, only the newest half of the lines is kept.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 512 * 1024;

static LOCK: Mutex<()> = Mutex::new(());

pub fn log_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("app.log")
}

/// Append a timestamped line to `app.log`, trimming the file first if it
/// has grown past the size cap. Never panics — logging failures are ignored.
pub fn log(msg: &str) {
    let _guard = LOCK.lock();
    let path = log_path();
    trim_if_needed(&path);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

fn trim_if_needed(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() <= MAX_BYTES {
        return;
    }
    if let Ok(content) = fs::read_to_string(path) {
        let total = content.lines().count();
        let kept: String = content
            .lines()
            .skip(total / 2)
            .flat_map(|l| [l, "\n"])
            .collect();
        let _ = fs::write(path, kept);
    }
}
