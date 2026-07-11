//! Configuration management — JSON persistence + Claude CLI auto-detect.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_claude_path")]
    pub claude_path: String,

    #[serde(default = "default_model")]
    pub default_model: String,

    #[serde(default = "default_check_interval")]
    pub check_interval_minutes: u32,

    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u32,

    #[serde(default = "default_message")]
    pub test_message: String,

    #[serde(default)]
    pub auto_start: bool,
}

fn default_claude_path() -> String { detect_claude_path() }
fn default_model() -> String { "haiku".into() }
fn default_check_interval() -> u32 { 60 }
fn default_cooldown() -> u32 { 60 }
fn default_message() -> String { "bu bir test mesajıdır".into() }

impl Default for Config {
    fn default() -> Self {
        Self {
            claude_path: detect_claude_path(),
            default_model: default_model(),
            check_interval_minutes: default_check_interval(),
            cooldown_seconds: default_cooldown(),
            test_message: default_message(),
            auto_start: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}

/// Auto-detect `claude.cmd` on Windows.
pub fn detect_claude_path() -> String {
    // 1. Common npm global install location
    if let Ok(appdata) = std::env::var("APPDATA") {
        let npm_path = PathBuf::from(&appdata).join("npm").join("claude.cmd");
        if npm_path.exists() {
            return npm_path.to_string_lossy().into_owned();
        }
    }

    // 2. Search PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            for name in &["claude.cmd", "claude.exe", "claude"] {
                let candidate = PathBuf::from(dir).join(name);
                if candidate.exists() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }

    // 3. Fallback
    "claude".into()
}
