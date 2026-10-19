//! Persisted user settings (the two source choices).
//!
//! Stored as a small JSON file in the platform config directory
//! (`%APPDATA%\KalkanMonitor\settings.json` on Windows). A missing or corrupt
//! file yields the defaults (Auto/Auto).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::selection::{FanSource, TempSource};

/// Which cooler the screen preview mimics. Alpha and Delta V2 speak the same
/// protocol, so this cannot be detected: the user picks it. Purely cosmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoolerModel {
    /// Round display with the temperature and the fan speed.
    DeltaV2,
    /// Round display with the temperature only (also any unknown value;
    /// `serde(other)` must be the last variant).
    #[default]
    #[serde(other)]
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub temp_source: TempSource,
    #[serde(default)]
    pub fan_source: FanSource,
    #[serde(default)]
    pub cooler_model: CoolerModel,
    /// The "start with the operating system" default was already applied once
    /// (so a user who turns it off later is never overridden).
    #[serde(default)]
    pub autostart_default_applied: bool,
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
            cooler_model: CoolerModel::DeltaV2,
            autostart_default_applied: true,
        };
        s.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p), s);
        let s2 = Settings { temp_source: TempSource::MaxCpuGpu, ..Settings::default() };
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
            Settings { temp_source: TempSource::MaxCpuGpu, ..Settings::default() }
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn cooler_model_defaults_and_back_compat() {
        let p = tmp("model");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // Old file without the field: Alpha, sources kept.
        std::fs::write(&p, r#"{"temp_source":"max_cpu_gpu"}"#).unwrap();
        assert_eq!(Settings::load_from(&p).cooler_model, CoolerModel::Alpha);
        assert_eq!(Settings::load_from(&p).temp_source, TempSource::MaxCpuGpu);
        // Old files never applied the autostart default.
        assert!(!Settings::load_from(&p).autostart_default_applied);
        // Unknown value falls back to Alpha without discarding the rest.
        std::fs::write(&p, r#"{"temp_source":"max_cpu_gpu","cooler_model":"bogus"}"#).unwrap();
        let s = Settings::load_from(&p);
        assert_eq!(s.cooler_model, CoolerModel::Alpha);
        assert_eq!(s.temp_source, TempSource::MaxCpuGpu);
        // Serialized form.
        std::fs::write(&p, r#"{"cooler_model":"delta_v2"}"#).unwrap();
        assert_eq!(Settings::load_from(&p).cooler_model, CoolerModel::DeltaV2);
        let json = serde_json::to_string(&Settings {
            cooler_model: CoolerModel::DeltaV2,
            ..Settings::default()
        })
        .unwrap();
        assert!(json.contains(r#""cooler_model":"delta_v2""#));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
