//! the terminal's icons: one clear symbol per icon id in an accent colour, where the gui
//! draws pixel art. the ids are the gui's (`icons::art` and `icons::small` in crates/ui):
//! desktop tiles show `tile`, title bars, the taskbar and menus show the one-cell `cell`.
//! the glyphs are box-drawing, block and geometric shapes, which every terminal font has.

use ratatui::buffer::Buffer;

use super::palette::{Palette, Rgb, BLACK, GRAY, NAVY, SHADOW, TEAL};
use super::{bold, put, style};

/// how one icon is drawn in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    /// the symbol on a desktop tile, at most four cells.
    pub tile: &'static str,
    /// the symbol in one cell, for title bars, buttons and menus.
    pub cell: &'static str,
    /// the colour the symbol is drawn in, on the gray face.
    pub accent: Rgb,
}

const BLUE: Rgb = (0, 0, 255);
const GREEN: Rgb = (0, 128, 0);
const MAROON: Rgb = (128, 0, 0);
const OLIVE: Rgb = (128, 128, 0);
const PURPLE: Rgb = (128, 0, 128);

const fn glyph(tile: &'static str, cell: &'static str, accent: Rgb) -> Glyph {
    Glyph { tile, cell, accent }
}

/// the glyph for an icon id; an unknown id gets a plain diamond.
pub fn glyph_for(id: &str) -> Glyph {
    match id {
        // a rack server: stacked drive bays
        "my-server" => glyph("≡≡", "≡", NAVY),
        // two networks joined
        "gateway" => glyph("◄►", "►", GREEN),
        // the shipping container
        "docker" => glyph("▓▓", "▓", BLUE),
        // a tower pc, the domain controller
        "nt4-domain" => glyph("▐▌", "▌", MAROON),
        // the globe
        "web-proxy" => glyph("◎", "◎", BLUE),
        // a command run somewhere else
        "remote-commands" => glyph("▶_", "▶", GREEN),
        // the name tree
        "dns" => glyph("┬┴", "┬", TEAL),
        // a prompt
        "console" => glyph(">_", ">", BLACK),
        // the machines on the network
        "hosts" => glyph("▣▣", "▣", TEAL),
        // a grid of switches
        "settings" => glyph("▦▦", "▦", PURPLE),
        "folder" => glyph("▬▬", "▬", OLIVE),
        // two linked screens
        "networks" => glyph("□─□", "□", NAVY),
        "lanlord" => glyph("L", "L", NAVY),
        "update" => glyph("▼", "▼", BLUE),
        "datetime" => glyph("◷", "◷", BLACK),
        _ => glyph("◆", "◆", SHADOW),
    }
}

/// an icon in one cell: its symbol in the accent colour on a gray chip, so it reads the same
/// on a navy title bar as on the gray taskbar.
pub fn chip(buf: &mut Buffer, x: u16, y: u16, id: &str, palette: Palette) {
    let g = glyph_for(id);
    put(buf, (x, y), g.cell, bold(style(palette, g.accent, GRAY)));
}

/// every id the gui has an icon for.
pub const IDS: [&str; 15] = [
    "gateway",
    "docker",
    "nt4-domain",
    "web-proxy",
    "remote-commands",
    "dns",
    "console",
    "hosts",
    "settings",
    "folder",
    "my-server",
    "networks",
    "lanlord",
    "update",
    "datetime",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn width(s: &str) -> usize {
        s.chars().count()
    }

    #[test]
    fn every_gui_icon_has_its_own_glyph() {
        let fallback = glyph_for("no such icon");
        for id in IDS {
            let g = glyph_for(id);
            assert_ne!(g, fallback, "{id} falls back");
            assert!((1..=4).contains(&width(g.tile)), "{id}: tile fits the face");
            assert_eq!(width(g.cell), 1, "{id}: one cell");
            assert!(
                g.tile.contains(g.cell),
                "{id}: the cell is part of the tile"
            );
        }
    }

    #[test]
    fn the_desktop_icons_are_told_apart_by_their_tiles() {
        let mut seen: Vec<&str> = IDS.iter().map(|id| glyph_for(id).tile).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), IDS.len());
    }

    #[test]
    fn no_emoji_or_wide_characters() {
        for id in IDS {
            for c in glyph_for(id).tile.chars() {
                // emoji and cjk start above the geometric shapes block
                assert!(u32::from(c) < 0x2600, "{id}: {c} may draw two cells wide");
            }
        }
    }
}
