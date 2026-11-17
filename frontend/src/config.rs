use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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
    pub last_game: String,
    pub recent_games: Vec<String>,
    pub controller_guid: String,
    pub port1_device: Option<u32>,
    pub fullscreen: bool,
    pub input: InputConfig,
    pub core_options: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        let data = data_dir().join("swanstation-ui");
        Self {
            core_path: PathBuf::new(),
            bios_dir: data.join("bios"),
            save_dir: data.join("saves"),
            state_dir: data.join("states"),
            last_game: String::new(),
            recent_games: Vec::new(),
            controller_guid: String::new(),
            port1_device: None,
            fullscreen: false,
            input: InputConfig::default(),
            core_options: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("swanstation-ui").join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Self>(&text) {
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
            },
            Err(_) => {
                let mut cfg = Self::default();
                cfg.ensure_dirs();
                cfg
            }
        }
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
        if let Ok(p) = env::var("SWANSTATION_CORE") {
            let p = PathBuf::from(p);
            if p.exists() {
                return Some(p);
            }
        }

        let mut candidates = Vec::new();
        if let Ok(exe) = env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("swanstation_libretro.so"));
                // Running the debug or release binary from frontend/target/...
                candidates.push(dir.join("../../../swanstation_libretro.so"));
                candidates.push(dir.join("../../../../swanstation_libretro.so"));
            }
        }
        if let Ok(cwd) = env::current_dir() {
            candidates.push(cwd.join("swanstation_libretro.so"));
            candidates.push(cwd.join("../swanstation_libretro.so"));
            candidates.push(cwd.join("../../swanstation_libretro.so"));
        }
        candidates.push(PathBuf::from("/usr/lib/libretro/swanstation_libretro.so"));
        candidates
            .into_iter()
            .find(|p| p.exists())
            .map(|p| fs::canonicalize(&p).unwrap_or(p))
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    home_dir().join(".local").join("share")
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
