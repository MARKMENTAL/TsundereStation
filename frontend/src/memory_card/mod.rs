//! PS1 memory-card support adapted from JAPS1ME (GPL-3.0-or-later),
//! Copyright (C) 2026 Mark Robillard Jr (MARKMENTAL).

pub mod card;
pub mod delete;
pub mod gme;
pub mod icon;
pub mod icon_png;
pub mod import;
pub mod io;
pub mod title;
mod ui;

pub use ui::MemoryCardManager;
