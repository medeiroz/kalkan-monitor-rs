//! Persisted user settings (the two source choices).
//!
//! Stored as a small JSON file in the platform config directory
//! (`%APPDATA%\KalkanMonitor\settings.json` on Windows). A missing or corrupt
//! file yields the defaults (Auto/Auto).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::selection::{FanSource, TempSource};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub temp_source: TempSource,
    #[serde(default)]
    pub fan_source: FanSource,
}

impl Settings {
    /// Location of the settings file, if the platform has a config directory.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("KalkanMonitor").join("settings.json"))
    }

    /// Loads the settings; any problem means the defaults.
    pub fn load() -> Self {
        Self::path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    /// Saves the settings to the default location.
    pub fn save(&self) -> Result<()> {
        match Self::path() {
            Some(p) => self.save_to(&p),
            None => Ok(()),
        }
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kalkan-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("settings.json")
    }

    #[test]
    fn roundtrip() {
        let p = tmp("roundtrip");
        let s = Settings {
            temp_source: TempSource::Sensor("/gpu/0/temp/0".into()),
            fan_source: FanSource::Sensor("/lpc/0/fan/2".into()),
        };
        s.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p), s);
        let s2 = Settings { temp_source: TempSource::MaxCpuGpu, fan_source: FanSource::Auto };
        s2.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p), s2);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn missing_or_corrupt_is_default() {
        let p = tmp("corrupt");
        assert_eq!(Settings::load_from(&p), Settings::default());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&p), Settings::default());
        std::fs::write(&p, r#"{"temp_source":"bogus"}"#).unwrap();
        assert_eq!(Settings::load_from(&p), Settings::default());
        // Partial file: the missing field is Auto.
        std::fs::write(&p, r#"{"temp_source":"max_cpu_gpu"}"#).unwrap();
        assert_eq!(
            Settings::load_from(&p),
            Settings { temp_source: TempSource::MaxCpuGpu, fan_source: FanSource::Auto }
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
