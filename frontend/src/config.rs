use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "tsunderestation";
const LEGACY_APP_DIR: &str = "swanstation-ui";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// "guide" or "select_start"
    pub menu_button: String,
    pub pause_on_menu: bool,
    pub deadzone: f32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            menu_button: "guide".to_string(),
            pause_on_menu: true,
            deadzone: 0.15,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub core_path: PathBuf,
    pub bios_dir: PathBuf,
    pub save_dir: PathBuf,
    pub state_dir: PathBuf,
    pub memory_card_source_dir: PathBuf,
    pub last_game: String,
    pub recent_games: Vec<String>,
    pub controller_guid: String,
    pub controller_guid_p2: String,
    pub port1_device: Option<u32>,
    pub port2_device: Option<u32>,
    pub fullscreen: bool,
    pub show_splash: bool,
    pub splash_seconds: f32,
    pub show_fps: bool,
    pub theme: String,
    pub input: InputConfig,
    pub core_options: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        let data = data_dir().join(APP_DIR);
        Self {
            core_path: PathBuf::new(),
            bios_dir: data.join("bios"),
            save_dir: data.join("saves"),
            state_dir: data.join("states"),
            memory_card_source_dir: PathBuf::new(),
            last_game: String::new(),
            recent_games: Vec::new(),
            controller_guid: String::new(),
            controller_guid_p2: String::new(),
            port1_device: None,
            port2_device: None,
            fullscreen: false,
            show_splash: true,
            splash_seconds: 4.0,
            show_fps: false,
            theme: "JAPS1ME".to_string(),
            input: InputConfig::default(),
            core_options: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join(APP_DIR).join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        if let Ok(text) = fs::read_to_string(&path) {
            return Self::parse_or_default(&text, &path);
        }
        if let Some(cfg) = Self::migrate_legacy() {
            return cfg;
        }
        let mut cfg = Self::default();
        cfg.ensure_dirs();
        cfg
    }

    fn parse_or_default(text: &str, path: &Path) -> Self {
        match toml::from_str::<Self>(text) {
            Ok(mut cfg) => {
                cfg.ensure_dirs();
                cfg
            }
            Err(e) => {
                log::warn!("Failed to parse {}: {e}; using defaults", path.display());
                let mut cfg = Self::default();
                cfg.ensure_dirs();
                cfg
            }
        }
    }

    /// One-time migration from the old `swanstation-ui` frontend identity.
    fn migrate_legacy() -> Option<Self> {
        let old_config = config_dir().join(LEGACY_APP_DIR).join("config.toml");
        if !old_config.exists() {
            return None;
        }
        let text = fs::read_to_string(&old_config).ok()?;
        let mut cfg = match toml::from_str::<Self>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                log::warn!(
                    "Could not parse legacy config {}: {e}; using defaults",
                    old_config.display()
                );
                Self::default()
            }
        };

        let old_root = data_dir().join(LEGACY_APP_DIR);
        let new_root = data_dir().join(APP_DIR);
        let mut use_new_paths = true;

        if old_root.exists() && !new_root.exists() {
            match fs::rename(&old_root, &new_root) {
                Ok(()) => log::info!(
                    "Migrated legacy data directory {} -> {}",
                    old_root.display(),
                    new_root.display()
                ),
                Err(rename_error) => {
                    log::warn!(
                        "Could not rename {} to {}: {rename_error}; trying a copy",
                        old_root.display(),
                        new_root.display()
                    );
                    match copy_dir_recursive(&old_root, &new_root) {
                        Ok(()) => log::info!(
                            "Copied legacy data directory {} -> {}",
                            old_root.display(),
                            new_root.display()
                        ),
                        Err(copy_error) => {
                            log::warn!(
                                "Could not copy legacy data; keeping old paths: {copy_error}"
                            );
                            use_new_paths = false;
                        }
                    }
                }
            }
        }

        if use_new_paths {
            cfg.bios_dir = rewrite_legacy_path(&cfg.bios_dir, &old_root, &new_root);
            cfg.save_dir = rewrite_legacy_path(&cfg.save_dir, &old_root, &new_root);
            cfg.state_dir = rewrite_legacy_path(&cfg.state_dir, &old_root, &new_root);
        }

        cfg.ensure_dirs();
        if let Err(e) = cfg.save() {
            log::warn!("Could not write migrated config: {e}");
        } else {
            log::info!("Migrated legacy config to {}", Self::path().display());
        }
        Some(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config directory {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn ensure_dirs(&mut self) {
        for dir in [&self.bios_dir, &self.save_dir, &self.state_dir] {
            if let Err(e) = fs::create_dir_all(dir) {
                log::warn!("Could not create {}: {e}", dir.display());
            }
        }
    }

    pub fn add_recent(&mut self, game: &Path) {
        let s = game.to_string_lossy().to_string();
        self.recent_games.retain(|x| x != &s);
        self.recent_games.insert(0, s);
        self.recent_games.truncate(10);
    }

    pub fn resolve_core(&self) -> Option<PathBuf> {
        if !self.core_path.as_os_str().is_empty() && self.core_path.exists() {
            return Some(self.core_path.clone());
        }
        for var in ["TSUNDERESTATION_CORE", "SWANSTATION_CORE"] {
            if let Ok(p) = env::var(var) {
                let p = PathBuf::from(p);
                if p.exists() {
                    return Some(p);
                }
            }
        }

        let mut candidates = Vec::new();
        if let Ok(exe) = env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("swanstation_libretro.so"));
                // Running the debug or release binary from frontend/target/...
                candidates.push(dir.join("../../../swanstation_libretro.so"));
                candidates.push(dir.join("../../../../swanstation_libretro.so"));
                // User-local install layout: ~/.local/bin/tsunderestation and
                // ~/.local/lib/libretro/swanstation_libretro.so.
                candidates.push(dir.join("../lib/libretro/swanstation_libretro.so"));
            }
        }
        if let Ok(cwd) = env::current_dir() {
            candidates.push(cwd.join("swanstation_libretro.so"));
            candidates.push(cwd.join("../swanstation_libretro.so"));
            candidates.push(cwd.join("../../swanstation_libretro.so"));
        }
        candidates.push(home_dir().join(".local/lib/libretro/swanstation_libretro.so"));
        candidates.push(PathBuf::from("/usr/lib/libretro/swanstation_libretro.so"));
        candidates
            .into_iter()
            .find(|p| p.exists())
            .map(|p| fs::canonicalize(&p).unwrap_or(p))
    }
}

fn rewrite_legacy_path(path: &Path, old_root: &Path, new_root: &Path) -> PathBuf {
    match path.strip_prefix(old_root) {
        Ok(rest) => new_root.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

pub fn data_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    home_dir().join(".local").join("share")
}

pub fn screenshots_dir() -> PathBuf {
    data_dir().join(APP_DIR).join("screenshots")
}

pub fn config_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    home_dir().join(".config")
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_without_player2_fields_parses() {
        let config: Config = toml::from_str("controller_guid = \"p1-guid\"\nport1_device = 258\n")
            .expect("legacy config should parse");
        assert_eq!(config.controller_guid, "p1-guid");
        assert_eq!(config.port1_device, Some(258));
        assert!(config.controller_guid_p2.is_empty());
        assert_eq!(config.port2_device, None);
    }

    #[test]
    fn player2_fields_roundtrip() {
        let config = Config {
            controller_guid: "p1-guid".into(),
            controller_guid_p2: "p2-guid".into(),
            port1_device: Some(258),
            port2_device: Some(261),
            ..Config::default()
        };

        let text = toml::to_string(&config).expect("serialize config");
        let parsed: Config = toml::from_str(&text).expect("parse config");
        assert_eq!(parsed.controller_guid, "p1-guid");
        assert_eq!(parsed.controller_guid_p2, "p2-guid");
        assert_eq!(parsed.port1_device, Some(258));
        assert_eq!(parsed.port2_device, Some(261));
    }
}
