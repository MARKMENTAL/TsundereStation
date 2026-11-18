// Adapted from JAPS1ME (GPL-3.0-or-later), Copyright (C) 2026 Mark Robillard Jr (MARKMENTAL).

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::card::parse_raw;
use super::gme::{parse_gme, GME_FILE_LEN, GME_HEADER_LEN, RAW_CARD_LEN};

/// A parsed card, always normalized to a 131072-byte raw image.
#[derive(Debug, Clone)]
pub struct LoadedCard {
    pub path: PathBuf,
    pub format: CardFormat,
    pub raw: Vec<u8>,
    pub summary: super::card::CardSummary,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardFormat {
    Raw,
    Gme,
}

impl CardFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw card",
            Self::Gme => "DexDrive .gme",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Gme => "gme",
        }
    }
}

/// The emulator whose memory-card directory is being browsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmulatorKind {
    TsundereStation,
    DuckStation,
    RetroArch,
}

impl EmulatorKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::TsundereStation => "TsundereStation",
            Self::DuckStation => "DuckStation",
            Self::RetroArch => "RetroArch",
        }
    }

    /// Infer the emulator from conventional directory components.
    pub fn detect(dir: &Path) -> Self {
        let components: Vec<String> = dir
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(value) => {
                    Some(value.to_string_lossy().to_ascii_lowercase())
                }
                _ => None,
            })
            .collect();
        if components
            .iter()
            .any(|component| component.contains("tsunderestation"))
        {
            Self::TsundereStation
        } else if components
            .iter()
            .any(|component| component == "memcards" || component.contains("duckstation"))
        {
            Self::DuckStation
        } else {
            Self::RetroArch
        }
    }
}

/// Load a card from disk, detecting `.gme` by size/signature and normalizing
/// it to a raw image. The returned summary warnings include both header and
/// directory warnings.
pub fn load_card(path: &Path) -> Result<LoadedCard> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let format = detect_format(&bytes).with_context(|| path.display().to_string())?;
    let mut warnings = Vec::new();
    let raw = match format {
        CardFormat::Gme => {
            let (card, header_warnings) = parse_gme(&bytes)?;
            warnings.extend(header_warnings);
            card.raw
        }
        CardFormat::Raw => bytes,
    };
    let summary = parse_raw(&raw).with_context(|| format!("parsing {}", path.display()))?;
    warnings.extend(summary.warnings.iter().cloned());
    Ok(LoadedCard {
        path: path.to_path_buf(),
        format,
        raw,
        summary,
        warnings,
    })
}

fn detect_format(bytes: &[u8]) -> Result<CardFormat> {
    if bytes.len() == RAW_CARD_LEN && bytes.starts_with(b"MC") {
        return Ok(CardFormat::Raw);
    }
    if bytes.len() == GME_FILE_LEN
        && bytes.len() >= GME_HEADER_LEN + 2
        && bytes[GME_HEADER_LEN..].starts_with(b"MC")
    {
        return Ok(CardFormat::Gme);
    }
    bail!(
        "unsupported memory-card format: expected a raw {} byte .srm/.mcd/.mcr image or a {} byte DexDrive .gme",
        RAW_CARD_LEN,
        GME_FILE_LEN
    )
}

/// List candidate card files in a directory, sorted by filename.
pub fn scan_cards(dir: &Path) -> Result<Vec<PathBuf>> {
    if dir.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    let mut cards = Vec::new();
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(extension.as_str(), "srm" | "mcd" | "mcr" | "gme") {
            cards.push(path);
        }
    }
    cards.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    });
    Ok(cards)
}

/// Default external emulator card directory: an existing DuckStation or
/// RetroArch directory, or an empty path when neither exists.
pub fn default_source_dir() -> PathBuf {
    for candidate in [duckstation_dir(), retroarch_dir()] {
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::new()
}

pub fn duckstation_dir() -> PathBuf {
    let xdg = crate::config::data_dir()
        .join("duckstation")
        .join("memcards");
    if xdg.is_dir() {
        return xdg;
    }
    let flatpak = crate::config::home_dir()
        .join(".var/app/org.duckstation.DuckStation/data/duckstation/memcards");
    if flatpak.is_dir() {
        return flatpak;
    }
    crate::config::data_dir()
        .join("duckstation")
        .join("memcards")
}

pub fn retroarch_dir() -> PathBuf {
    let config = crate::config::config_dir().join("retroarch").join("saves");
    if config.is_dir() {
        return config;
    }
    let data = crate::config::data_dir().join("retroarch").join("saves");
    if data.is_dir() {
        return data;
    }
    config
}

/// Default destination name when copying a card into a TsundereStation save
/// directory. Raw card layouts are identical; only the extension/naming
/// convention changes.
pub fn local_copy_name(source: &Path) -> String {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "memory-card".to_string());
    format!("{}.srm", strip_duckstation_slot(&stem))
}

/// Strip a trailing DuckStation `_SLOT` suffix (`_1`, `_2`, ...).
pub fn strip_duckstation_slot(stem: &str) -> String {
    if let Some((base, suffix)) = stem.rsplit_once('_') {
        if !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && suffix.parse::<u32>().is_ok_and(|slot| slot > 0)
        {
            return base.to_string();
        }
    }
    stem.to_string()
}

/// Detect conventional multi-disc labels without matching words like
/// `Discovery`.
pub fn is_multi_disc(game_region: &str) -> bool {
    let lower = game_region.to_ascii_lowercase();
    for token in ["disc", "disk"] {
        let mut rest = lower.as_str();
        while let Some(index) = rest.find(token) {
            let before = rest[..index].chars().next_back();
            let suffix = &rest[index + token.len()..];
            let after_token = suffix.trim_start();
            let has_number = after_token
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_digit());
            let token_boundary = before.is_none_or(|ch| !ch.is_ascii_alphabetic());
            if token_boundary && has_number {
                return true;
            }
            let Some(next) = suffix.chars().next() else {
                break;
            };
            rest = &suffix[next.len_utf8()..];
            if rest.is_empty() {
                break;
            }
        }
    }
    false
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn backup_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".japs1me-bak");
    target.with_file_name(name)
}

pub fn backup_file_atomically(target: &Path, backup: &Path) -> Result<()> {
    match backup.symlink_metadata() {
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!(
                "backup path exists and is not a regular file: {}",
                backup.display()
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("checking {}", backup.display())),
    }
    let bytes = std::fs::read(target)
        .with_context(|| format!("reading {} for backup", target.display()))?;
    write_temp_then_persist(backup, &bytes)
        .with_context(|| format!("backing up {} to {}", target.display(), backup.display()))
}

/// Write a new file, refusing to replace an existing path.
pub fn write_new_file_atomically(path: &Path, data: &[u8]) -> Result<()> {
    if path.symlink_metadata().is_ok() {
        bail!("output already exists: {}", path.display());
    }
    write_temp_then_persist(path, data)
}

/// Replace an existing file via a staged temporary file.
pub fn replace_file_atomically(path: &Path, data: &[u8]) -> Result<()> {
    write_temp_then_persist(path, data)
}

/// Replace a card and back up the old version when it exists.
pub fn replace_with_backup(path: &Path, data: &[u8]) -> Result<Option<PathBuf>> {
    let backup = if path.symlink_metadata().is_ok() {
        let backup = backup_path(path);
        backup_file_atomically(path, &backup)?;
        Some(backup)
    } else {
        None
    };
    replace_file_atomically(path, data)?;
    Ok(backup)
}

/// Copy a card from any supported source to a destination, normalizing `.gme`
/// to raw bytes and verifying the destination.
pub fn copy_card(source: &Path, destination: &Path) -> Result<Option<PathBuf>> {
    if same_file(source, destination) {
        bail!(
            "source and destination are the same file: {}",
            destination.display()
        );
    }
    let loaded = load_card(source)?;
    let backup = if destination.symlink_metadata().is_ok() {
        replace_with_backup(destination, &loaded.raw)?
    } else {
        write_new_file_atomically(destination, &loaded.raw)?;
        None
    };
    parse_raw(
        &std::fs::read(destination)
            .with_context(|| format!("reading {}", destination.display()))?,
    )
    .with_context(|| format!("verifying {}", destination.display()))?;
    Ok(backup)
}

fn write_temp_then_persist(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    temp.write_all(data)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;
    temp.persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_duckstation_slots() {
        assert_eq!(strip_duckstation_slot("Game (USA)_1"), "Game (USA)");
        assert_eq!(strip_duckstation_slot("Game (USA)_12"), "Game (USA)");
        assert_eq!(strip_duckstation_slot("Game_0"), "Game_0");
        assert_eq!(strip_duckstation_slot("Game_test"), "Game_test");
    }

    #[test]
    fn detects_emulators_from_paths() {
        assert_eq!(
            EmulatorKind::detect(Path::new("/home/me/.local/share/duckstation/memcards")),
            EmulatorKind::DuckStation
        );
        assert_eq!(
            EmulatorKind::detect(Path::new("/home/me/.local/share/tsunderestation/saves")),
            EmulatorKind::TsundereStation
        );
        assert_eq!(
            EmulatorKind::detect(Path::new("/home/me/.config/retroarch/saves")),
            EmulatorKind::RetroArch
        );
    }

    #[test]
    fn makes_local_copy_names() {
        assert_eq!(
            local_copy_name(Path::new("Game (USA)_2.mcd")),
            "Game (USA).srm"
        );
    }

    #[test]
    fn detects_multi_disc_labels() {
        assert!(is_multi_disc("Game (Disc 1)"));
        assert!(is_multi_disc("Game (Disk 2)"));
        assert!(!is_multi_disc("Discovery"));
    }

    fn blank_raw() -> Vec<u8> {
        let mut raw = vec![0u8; RAW_CARD_LEN];
        raw[0] = b'M';
        raw[1] = b'C';
        raw
    }

    #[test]
    fn copies_gme_and_raw_cards_with_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = blank_raw();
        let source = dir.path().join("source.gme");
        let mut gme = vec![0u8; GME_HEADER_LEN];
        gme.extend_from_slice(&raw);
        std::fs::write(&source, &gme).expect("write gme");

        let destination = dir.path().join("game.srm");
        let backup = copy_card(&source, &destination).expect("copies");
        assert!(backup.is_none());
        assert_eq!(std::fs::read(&destination).expect("read"), raw);

        std::fs::write(&destination, b"old").expect("write old");
        let backup = copy_card(&source, &destination)
            .expect("replaces")
            .expect("backup");
        assert_eq!(std::fs::read(&backup).expect("read backup"), b"old");
        assert_eq!(std::fs::read(&destination).expect("read"), raw);
    }

    #[test]
    fn new_output_refuses_overwrite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("game.srm");
        std::fs::write(&path, b"existing").expect("write");
        assert!(write_new_file_atomically(&path, b"new").is_err());
        assert_eq!(std::fs::read(&path).expect("read"), b"existing");
    }
}
