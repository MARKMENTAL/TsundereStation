use crate::libretro::video::Frame;
use anyhow::{Context, Result};
use chrono::Local;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// Save a JPEG screenshot using the local `YYYYMMDDHHMMSS` timestamp.
pub fn capture(frame: &Frame, game_name: &str, directory: &Path) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    save_with_timestamp(frame, game_name, directory, &timestamp)
}

/// Save a JPEG screenshot with an explicit timestamp. Exposed for tests and
/// for deterministic naming.
pub fn save_with_timestamp(
    frame: &Frame,
    game_name: &str,
    directory: &Path,
    timestamp: &str,
) -> Result<PathBuf> {
    if frame.width == 0 || frame.height == 0 || frame.rgba.len() < frame.width * frame.height * 4 {
        anyhow::bail!("cannot save an empty video frame");
    }
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating screenshot directory {}", directory.display()))?;

    let stem = sanitize_game_name(game_name);
    let mut path = directory.join(format!("{stem}-{timestamp}.jpg"));
    let mut suffix = 1;
    while path.exists() {
        path = directory.join(format!("{stem}-{timestamp}-{suffix}.jpg"));
        suffix += 1;
    }

    write_jpeg(frame, &path)?;
    Ok(path)
}

/// Make a game title safe as a filename without destroying normal punctuation
/// or non-ASCII characters.
pub fn sanitize_game_name(game_name: &str) -> String {
    let stem = if game_name.contains('/') || game_name.contains('\\') {
        game_name.to_string()
    } else {
        Path::new(game_name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| game_name.to_string())
    };
    let mut value = String::new();
    for ch in stem.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            value.push('_');
        } else {
            value.push(ch);
        }
    }
    let value = value.trim().trim_end_matches('.').trim();
    let mut value = if value.is_empty() {
        "screenshot".to_string()
    } else {
        value.to_string()
    };
    if value.chars().count() > 80 {
        value = value.chars().take(80).collect();
    }
    value
}

fn write_jpeg(frame: &Frame, path: &Path) -> Result<()> {
    let rgb: Vec<u8> = frame
        .rgba
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    let file = std::fs::File::create(path)
        .with_context(|| format!("creating screenshot {}", path.display()))?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(BufWriter::new(file), 92);
    encoder
        .encode(
            &rgb,
            frame.width as u32,
            frame.height as u32,
            image::ExtendedColorType::Rgb8,
        )
        .with_context(|| format!("encoding screenshot {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        }
    }

    #[test]
    fn sanitizes_game_names() {
        assert_eq!(sanitize_game_name("Game (USA).m3u"), "Game (USA)");
        assert_eq!(sanitize_game_name("bad/name: test"), "bad_name_ test");
        assert_eq!(sanitize_game_name(""), "screenshot");
        assert_eq!(sanitize_game_name("...."), "screenshot");
    }

    #[test]
    fn saves_jpeg_with_collision_suffix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first =
            save_with_timestamp(&frame(), "Game", dir.path(), "20261118123456").expect("first");
        assert!(first.ends_with("Game-20261118123456.jpg"));
        let second =
            save_with_timestamp(&frame(), "Game", dir.path(), "20261118123456").expect("second");
        assert!(second.ends_with("Game-20261118123456-1.jpg"));
        let bytes = std::fs::read(&first).expect("read jpeg");
        assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]));
    }
}
