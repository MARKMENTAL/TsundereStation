// Adapted from JAPS1ME (GPL-3.0-or-later), Copyright (C) 2026 Mark Robillard Jr (MARKMENTAL).
//! Raw PS1 memory-card parsing (`.srm`/`.mcr` and stripped `.gme` payloads).
//!
//! Layout (all offsets into the 131072-byte raw image):
//! - Bytes `0..128`: card header frame (`MC` magic).
//! - Bytes `128..2048`: 15 directory frames of 128 bytes.
//! - Bytes `8192..`: 15 save data blocks of 8192 bytes.
//!
//! Directory frame fields used here: type byte at 0, 3-byte little-endian
//! payload size at 4..7, next-slot pointer at 8, region at 10..12, product
//! code at 12..22, identifier at 22..30, and an XOR checksum at 127 covering
//! bytes `0..127` (see MemcardRex `calculateXOR`).

use crate::memory_card::gme::RAW_CARD_LEN;
use crate::memory_card::title::decode_title;
use anyhow::{bail, Result};

/// Number of save slots on a PS1 card.
pub const SLOT_COUNT: usize = 15;
/// Length of one directory frame.
pub const DIR_FRAME_LEN: usize = 128;
/// Length of one save data block.
pub const SAVE_BLOCK_LEN: usize = 8192;
/// Offset of the directory frames in the raw image.
pub(crate) const DIR_FRAMES_OFFSET: usize = 128;
/// Offset of the save data blocks in the raw image.
pub(crate) const SAVE_DATA_OFFSET: usize = 8192;

/// Classification of a directory-frame slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotType {
    Formatted,
    Initial,
    MiddleLink,
    EndLink,
    DeletedInitial,
    DeletedMiddleLink,
    DeletedEndLink,
    Corrupted,
}

impl SlotType {
    fn from_byte(byte: u8) -> Self {
        match byte {
            0xA0 => Self::Formatted,
            0x51 => Self::Initial,
            0x52 => Self::MiddleLink,
            0x53 => Self::EndLink,
            0xA1 => Self::DeletedInitial,
            0xA2 => Self::DeletedMiddleLink,
            0xA3 => Self::DeletedEndLink,
            _ => Self::Corrupted,
        }
    }

    pub(crate) fn is_initial(self) -> bool {
        matches!(self, Self::Initial | Self::DeletedInitial)
    }

    pub(crate) fn is_link(self) -> bool {
        matches!(
            self,
            Self::MiddleLink | Self::EndLink | Self::DeletedMiddleLink | Self::DeletedEndLink
        )
    }

    fn is_deleted(self) -> bool {
        matches!(
            self,
            Self::DeletedInitial | Self::DeletedMiddleLink | Self::DeletedEndLink
        )
    }
}

pub(crate) struct DirFrame {
    pub(crate) slot_type: SlotType,
    pub(crate) next: u8,
    pub(crate) region_raw: String,
    pub(crate) product_code: String,
    pub(crate) identifier: String,
}

impl DirFrame {
    /// Save name (`region + product code + identifier`).
    pub(crate) fn save_name(&self) -> String {
        format!(
            "{}{}{}",
            self.region_raw, self.product_code, self.identifier
        )
    }
}

/// One save (an initial slot plus its linked blocks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotInfo {
    /// Zero-based initial slot.
    pub slot: usize,
    /// Save name (`region + product code + identifier`, e.g. `BESLES-03946PES-OPT`).
    pub save_name: String,
    /// Human-readable region (`America`/`Europe`/`Japan`, else the raw code).
    pub region: String,
    /// Game title decoded from the save data (falls back to the save name).
    pub game_title: String,
    /// Number of slots (blocks) the save occupies.
    pub blocks: usize,
    /// Payload size in bytes (`blocks * 8192`).
    pub size_bytes: usize,
    /// True when the save is marked deleted.
    pub deleted: bool,
}

/// Parsed card: saves in slot order plus free-slot count and warnings.
#[derive(Debug, Clone)]
pub struct CardSummary {
    pub saves: Vec<SlotInfo>,
    pub free_slots: usize,
    pub warnings: Vec<String>,
}

/// Parse a 131072-byte raw card image.
pub fn parse_raw(raw: &[u8]) -> Result<CardSummary> {
    let (frames, mut warnings) = read_frames(raw)?;

    // Follow link chains from each initial slot.
    let mut reachable = [false; SLOT_COUNT];
    let mut saves = Vec::new();
    for slot in 0..SLOT_COUNT {
        if !frames[slot].slot_type.is_initial() {
            continue;
        }
        let chain = find_links(&frames, slot);
        for &linked in &chain {
            reachable[linked] = true;
        }
        let frame = &frames[slot];
        let save_name = frame.save_name();
        let data_start = SAVE_DATA_OFFSET + slot * SAVE_BLOCK_LEN;
        let title_field =
            &raw[data_start + 4..data_start + 4 + crate::memory_card::title::TITLE_LEN];
        let game_title = decode_title(title_field)
            .map(normalize_title)
            .unwrap_or_else(|| save_name.clone());
        let blocks = chain.len();
        saves.push(SlotInfo {
            slot,
            save_name,
            region: display_region(&frame.region_raw),
            game_title,
            blocks,
            size_bytes: blocks * SAVE_BLOCK_LEN,
            deleted: frame.slot_type.is_deleted(),
        });
    }

    // Slots never reached from an initial slot but typed as links hold stale
    // data (broken links); MemcardRex reclassifies those as free.
    let mut free_slots = 0;
    for slot in 0..SLOT_COUNT {
        match frames[slot].slot_type {
            SlotType::Formatted => free_slots += 1,
            slot_type if slot_type.is_link() && !reachable[slot] => {
                free_slots += 1;
                warnings.push(format!("slot {}: orphaned link treated as free", slot + 1));
            }
            _ => {}
        }
    }

    Ok(CardSummary {
        saves,
        free_slots,
        warnings,
    })
}

/// Normalize full-width Latin/space characters for the egui font. The card
/// data is untouched; this only affects how titles are displayed.
fn normalize_title(text: String) -> String {
    text.chars()
        .map(|ch| match ch {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch),
            _ => ch,
        })
        .collect()
}

/// Reset slots to formatted (frames plus zeroed data blocks), like
/// MemcardRex `FormatSave`. Freed slots read as free on re-parse.
pub(crate) fn format_slots(raw: &mut [u8], chain: &[usize]) {
    for &slot in chain {
        let frame = DIR_FRAMES_OFFSET + slot * DIR_FRAME_LEN;
        raw[frame..frame + DIR_FRAME_LEN].fill(0);
        raw[frame] = 0xA0;
        raw[frame + 8] = 0xFF;
        raw[frame + 9] = 0xFF;
        let mut xor = 0u8;
        for byte in &raw[frame..frame + DIR_FRAME_LEN - 1] {
            xor ^= *byte;
        }
        raw[frame + DIR_FRAME_LEN - 1] = xor;
        let data = SAVE_DATA_OFFSET + slot * SAVE_BLOCK_LEN;
        raw[data..data + SAVE_BLOCK_LEN].fill(0);
    }
}

/// Resolve a 1-based slot number or exact save name to the initial slot that
/// starts the save.
///
/// All-digit selections are slot numbers (which must start a save); anything
/// else is matched against save names. Shared by `delete` and `export-icon`.
pub(crate) fn find_save_start(frames: &[DirFrame], select: &str) -> Result<usize> {
    if let Ok(number) = select.trim().parse::<usize>() {
        if number == 0 || number > SLOT_COUNT {
            bail!("no save at slot '{select}': slots run 1..={SLOT_COUNT}");
        }
        if !frames[number - 1].slot_type.is_initial() {
            bail!("no save starts at slot {number}");
        }
        Ok(number - 1)
    } else {
        (0..SLOT_COUNT)
            .find(|&slot| frames[slot].slot_type.is_initial() && frames[slot].save_name() == select)
            .ok_or_else(|| anyhow::anyhow!("save '{select}' not found on card"))
    }
}

/// Read and validate the 15 directory frames of a raw card image.
///
/// Returns the frames plus non-fatal checksum warnings. Length and `MC`
/// signature failures are hard errors.
pub(crate) fn read_frames(raw: &[u8]) -> Result<(Vec<DirFrame>, Vec<String>)> {
    if raw.len() != RAW_CARD_LEN {
        bail!(
            "not a raw PS1 card image: expected {} bytes, found {}",
            RAW_CARD_LEN,
            raw.len()
        );
    }
    if raw[0] != b'M' || raw[1] != b'C' {
        bail!("not a raw PS1 card image: missing MC signature");
    }

    let mut frames = Vec::with_capacity(SLOT_COUNT);
    let mut warnings = Vec::new();
    for slot in 0..SLOT_COUNT {
        let start = DIR_FRAMES_OFFSET + slot * DIR_FRAME_LEN;
        let frame = &raw[start..start + DIR_FRAME_LEN];
        let mut xor = 0u8;
        for byte in &frame[..DIR_FRAME_LEN - 1] {
            xor ^= byte;
        }
        if xor != frame[DIR_FRAME_LEN - 1] {
            warnings.push(format!("slot {}: directory checksum mismatch", slot + 1));
        }
        frames.push(DirFrame {
            slot_type: SlotType::from_byte(frame[0]),
            next: frame[8],
            region_raw: decode_ascii(&frame[10..12]),
            product_code: decode_ascii(&frame[12..22]),
            identifier: decode_ascii(&frame[22..30]),
        });
    }
    Ok((frames, warnings))
}

/// Follow a save's link chain, with bounds mirroring MemcardRex `FindSaveLinks`.
pub(crate) fn find_links(frames: &[DirFrame], start: usize) -> Vec<usize> {
    let mut chain = vec![start];
    let mut current = start;
    for _ in 0..SLOT_COUNT {
        let next = frames[current].next;
        if next == 0xFF {
            break;
        }
        let next = next as usize;
        if next >= SLOT_COUNT {
            break;
        }
        if !frames[next].slot_type.is_link() {
            break;
        }
        let end = matches!(
            frames[next].slot_type,
            SlotType::EndLink | SlotType::DeletedEndLink
        );
        chain.push(next);
        current = next;
        if end {
            break;
        }
    }
    chain
}

fn decode_ascii(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end]
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn display_region(raw: &str) -> String {
    match raw {
        "BA" => "America".to_owned(),
        "BE" => "Europe".to_owned(),
        "BI" => "Japan".to_owned(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_raw() -> Vec<u8> {
        let mut raw = vec![0u8; RAW_CARD_LEN];
        raw[0] = b'M';
        raw[1] = b'C';
        for slot in 0..SLOT_COUNT {
            let start = DIR_FRAMES_OFFSET + slot * DIR_FRAME_LEN;
            raw[start] = 0xA0;
            raw[start + 8] = 0xFF;
            raw[start + 9] = 0xFF;
            fix_xor(&mut raw, start);
        }
        raw
    }

    fn fix_xor(raw: &mut [u8], start: usize) {
        let mut xor = 0u8;
        for byte in &raw[start..start + DIR_FRAME_LEN - 1] {
            xor ^= byte;
        }
        raw[start + DIR_FRAME_LEN - 1] = xor;
    }

    fn write_frame(raw: &mut [u8], slot: usize, frame: &[u8; DIR_FRAME_LEN]) {
        let start = DIR_FRAMES_OFFSET + slot * DIR_FRAME_LEN;
        raw[start..start + DIR_FRAME_LEN].copy_from_slice(frame);
    }

    fn initial_frame(next: u8, region: &[u8; 2], prod: &[u8], ident: &[u8]) -> [u8; DIR_FRAME_LEN] {
        let mut frame = [0u8; DIR_FRAME_LEN];
        frame[0] = 0x51;
        frame[4] = 0x00;
        frame[5] = 0x20;
        frame[8] = next;
        frame[9] = if next == 0xFF { 0xFF } else { 0x00 };
        frame[10..12].copy_from_slice(region);
        frame[12..12 + prod.len().min(10)].copy_from_slice(&prod[..prod.len().min(10)]);
        frame[22..22 + ident.len().min(8)].copy_from_slice(&ident[..ident.len().min(8)]);
        let mut xor = 0u8;
        for byte in &frame[..DIR_FRAME_LEN - 1] {
            xor ^= byte;
        }
        frame[DIR_FRAME_LEN - 1] = xor;
        frame
    }

    #[test]
    fn parses_empty_card() {
        let summary = parse_raw(&blank_raw()).expect("parses");
        assert!(summary.saves.is_empty());
        assert_eq!(summary.free_slots, SLOT_COUNT);
        assert!(summary.warnings.is_empty());
    }

    #[test]
    fn parses_single_block_save() {
        let mut raw = blank_raw();
        write_frame(
            &mut raw,
            0,
            &initial_frame(0xFF, b"BE", b"SLES-03946", b"PES-OPT"),
        );
        let data = SAVE_DATA_OFFSET;
        raw[data + 4..data + 11].copy_from_slice(b"PES-OPT");
        let summary = parse_raw(&raw).expect("parses");
        assert_eq!(summary.saves.len(), 1);
        let save = &summary.saves[0];
        assert_eq!(save.slot, 0);
        assert_eq!(save.save_name, "BESLES-03946PES-OPT");
        assert_eq!(save.region, "Europe");
        assert_eq!(save.game_title, "PES-OPT");
        assert_eq!(save.blocks, 1);
        assert_eq!(save.size_bytes, SAVE_BLOCK_LEN);
        assert!(!save.deleted);
        assert_eq!(summary.free_slots, SLOT_COUNT - 1);
    }

    #[test]
    fn follows_multi_block_links() {
        let mut raw = blank_raw();
        write_frame(
            &mut raw,
            0,
            &initial_frame(1, b"BA", b"SLUS-12345", b"SAVE1"),
        );
        let mut end = [0u8; DIR_FRAME_LEN];
        end[0] = 0x53;
        end[8] = 0xFF;
        end[9] = 0xFF;
        let mut xor = 0u8;
        for byte in &end[..DIR_FRAME_LEN - 1] {
            xor ^= byte;
        }
        end[DIR_FRAME_LEN - 1] = xor;
        write_frame(&mut raw, 1, &end);
        let summary = parse_raw(&raw).expect("parses");
        assert_eq!(summary.saves.len(), 1);
        assert_eq!(summary.saves[0].blocks, 2);
        assert_eq!(summary.free_slots, SLOT_COUNT - 2);
    }

    #[test]
    fn reports_bad_xor_without_failing() {
        let mut raw = blank_raw();
        let start = DIR_FRAMES_OFFSET;
        raw[start + 127] ^= 0xFF;
        let summary = parse_raw(&raw).expect("still parses");
        assert_eq!(summary.warnings.len(), 1);
        assert!(summary.warnings[0].contains("checksum mismatch"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_raw(&[0u8; 100]).is_err());
        assert!(parse_raw(&[0u8; RAW_CARD_LEN]).is_err());
    }

    #[test]
    fn normalizes_full_width_titles_for_display() {
        assert_eq!(
            normalize_title("ＰｒｏＥｖｏｌｕｔｉｏｎ\u{3000}２".to_string()),
            "ProEvolution 2"
        );
    }
}
