// Adapted from JAPS1ME (GPL-3.0-or-later), Copyright (C) 2026 Mark Robillard Jr (MARKMENTAL).
//! PS1 save-icon decoding (16x16, 4bpp) into RGBA pixels.
//!
//! In a save's initial data block:
//! - Palette: 32 bytes at block offset `0x60` (16 x 16-bit BGR555 entries).
//! - Icon bitmap: 128 bytes at block offset `0x80` (16 rows of 8 bytes; the
//!   low nibble is the left pixel and the high nibble the right pixel).
//!
//! A palette entry whose red, green, blue, and bit-15 flag are all zero is
//! treated as transparent, matching MemcardRex's `loadPalette`/`loadIcons`.

use crate::memory_card::card::{find_save_start, read_frames, SAVE_BLOCK_LEN, SAVE_DATA_OFFSET};
use anyhow::Result;

/// Palette offset within a save data block.
pub const PALETTE_OFFSET: usize = 0x60;
/// Palette length in bytes (16 colors x 2 bytes).
pub const PALETTE_LEN: usize = 32;
/// Offset of the primary icon bitmap within a save data block.
pub const ICON_BITMAP_OFFSET: usize = 0x80;
/// Icon bitmap length in bytes (16x16 pixels at 4bpp).
pub const ICON_BITMAP_LEN: usize = 128;
/// Icon width and height in pixels.
pub const ICON_DIM: usize = 16;

/// A decoded icon as 8-bit RGBA pixels, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width * height` RGBA quadbytes.
    pub rgba: Vec<u8>,
    /// Decoded 16-color palette in row-major RGBA bytes.
    pub palette: [[u8; 4]; 16],
    /// Original palette index for each pixel, row-major.
    pub palette_indices: Vec<u8>,
}

/// Decode the primary 16x16 icon for the save selected by 1-based slot number
/// or exact save name.
pub fn decode_icon(raw: &[u8], select: &str) -> Result<IconImage> {
    let (frames, _) = read_frames(raw)?;
    let slot = find_save_start(&frames, select)?;

    let block_start = SAVE_DATA_OFFSET + slot * SAVE_BLOCK_LEN;
    let block = &raw[block_start..block_start + SAVE_BLOCK_LEN];
    let palette = decode_palette(&block[PALETTE_OFFSET..PALETTE_OFFSET + PALETTE_LEN]);
    let bitmap = &block[ICON_BITMAP_OFFSET..ICON_BITMAP_OFFSET + ICON_BITMAP_LEN];

    let mut rgba = Vec::with_capacity(ICON_DIM * ICON_DIM * 4);
    let mut palette_indices = Vec::with_capacity(ICON_DIM * ICON_DIM);
    for row in bitmap.chunks_exact(ICON_DIM / 2) {
        for &byte in row {
            let low = byte & 0x0F;
            let high = byte >> 4;
            rgba.extend_from_slice(&palette[low as usize]);
            rgba.extend_from_slice(&palette[high as usize]);
            palette_indices.extend_from_slice(&[low, high]);
        }
    }

    Ok(IconImage {
        width: ICON_DIM,
        height: ICON_DIM,
        rgba,
        palette,
        palette_indices,
    })
}

/// Decode the 16-entry palette into RGBA colors.
fn decode_palette(bytes: &[u8]) -> [[u8; 4]; 16] {
    let mut palette = [[0u8; 4]; 16];
    for (index, chunk) in bytes.chunks_exact(2).enumerate() {
        let low = chunk[0];
        let high = chunk[1];
        let red = (low & 0x1F) << 3;
        let green = ((high & 0x03) << 6) | ((low & 0xE0) >> 2);
        let blue = (high & 0x7C) << 1;
        let opaque = (red | green | blue | (high & 0x80)) != 0;
        palette[index] = [red, green, blue, if opaque { 0xFF } else { 0x00 }];
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_card::card::{DIR_FRAMES_OFFSET, DIR_FRAME_LEN, SLOT_COUNT};
    use crate::memory_card::gme::RAW_CARD_LEN;

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
            xor ^= *byte;
        }
        raw[start + DIR_FRAME_LEN - 1] = xor;
    }

    fn make_save(raw: &mut [u8], slot: usize, region: &[u8; 2], prod: &[u8], ident: &[u8]) {
        let start = DIR_FRAMES_OFFSET + slot * DIR_FRAME_LEN;
        raw[start] = 0x51;
        raw[start + 4] = 0x00;
        raw[start + 5] = 0x20;
        raw[start + 8] = 0xFF;
        raw[start + 9] = 0xFF;
        raw[start + 10..start + 12].copy_from_slice(region);
        raw[start + 12..start + 12 + prod.len().min(10)]
            .copy_from_slice(&prod[..prod.len().min(10)]);
        raw[start + 22..start + 22 + ident.len().min(8)]
            .copy_from_slice(&ident[..ident.len().min(8)]);
        fix_xor(raw, start);
    }

    fn data_block(raw: &mut [u8], slot: usize) -> &mut [u8] {
        let start = SAVE_DATA_OFFSET + slot * SAVE_BLOCK_LEN;
        &mut raw[start..start + SAVE_BLOCK_LEN]
    }

    #[test]
    fn decodes_palette_and_pixels() {
        let mut raw = blank_raw();
        make_save(&mut raw, 0, b"BE", b"SLES-03946", b"PES-OPT");
        let block = data_block(&mut raw, 0);

        // Entry 0 transparent, entry 1 red, entry 2 green.
        block[PALETTE_OFFSET..PALETTE_OFFSET + 6]
            .copy_from_slice(&[0x00, 0x00, 0x1F, 0x00, 0xE0, 0x03]);
        // Pixel (0,0) = index 1 (red), pixel (1,0) = index 0 (transparent).
        block[ICON_BITMAP_OFFSET] = 0x01;

        let icon = decode_icon(&raw, "BESLES-03946PES-OPT").expect("decodes");
        assert_eq!(icon.width, 16);
        assert_eq!(icon.height, 16);
        assert_eq!(icon.rgba.len(), 16 * 16 * 4);
        assert_eq!(icon.palette_indices.len(), 16 * 16);
        assert_eq!(&icon.rgba[0..4], &[248, 0, 0, 255]);
        assert_eq!(&icon.rgba[4..8], &[0, 0, 0, 0]);
        assert_eq!(&icon.palette_indices[0..2], &[1, 0]);
    }

    #[test]
    fn selects_by_slot_number() {
        let mut raw = blank_raw();
        make_save(&mut raw, 0, b"BE", b"SLES-03946", b"PES-OPT");
        make_save(&mut raw, 3, b"BA", b"SLUS-12345", b"SAVE1");
        let block = data_block(&mut raw, 3);
        block[PALETTE_OFFSET + 2..PALETTE_OFFSET + 4].copy_from_slice(&[0x1F, 0x00]);
        block[ICON_BITMAP_OFFSET] = 0x01;

        let icon = decode_icon(&raw, "4").expect("decodes by slot");
        assert_eq!(&icon.rgba[0..4], &[248, 0, 0, 255]);
    }

    #[test]
    fn rejects_unknown_selection() {
        let mut raw = blank_raw();
        make_save(&mut raw, 0, b"BE", b"SLES-03946", b"PES-OPT");
        assert!(decode_icon(&raw, "BENOPE-00000NOPE").is_err());
        assert!(decode_icon(&raw, "2").is_err());
        assert!(decode_icon(&raw, "0").is_err());
    }
}
