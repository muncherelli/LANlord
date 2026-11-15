//! pixel-art icons, 16 by 16, drawn as scaled squares.

use eframe::egui::{Color32, Painter, Pos2, Rect, Vec2};

/// one palette letter per pixel; `.` is transparent.
fn colour(c: char) -> Option<Color32> {
    Some(match c {
        'k' => Color32::BLACK,
        'w' => Color32::WHITE,
        'g' => Color32::from_rgb(192, 192, 192),
        'd' => Color32::from_rgb(128, 128, 128),
        'n' => Color32::from_rgb(0, 0, 128),
        'b' => Color32::from_rgb(0, 0, 255),
        'c' => Color32::from_rgb(0, 255, 255),
        't' => Color32::from_rgb(0, 128, 128),
        'G' => Color32::from_rgb(0, 192, 0),
        'y' => Color32::from_rgb(255, 255, 0),
        'r' => Color32::from_rgb(255, 0, 0),
        _ => return None,
    })
}

const GATEWAY: [&str; 16] = [
    "................",
    "................",
    "..kkkkkkkkkkkk..",
    ".kwwwwwwwwwwwwk.",
    ".kwggggggggggdk.",
    ".kwgGgGgGgggkdk.",
    ".kwggggggggggdk.",
    ".kwdddddddddddk.",
    ".kkkkkkkkkkkkkk.",
    "..kdddddddddddk.",
    "...kkkkkkkkkkk..",
    "......kddk......",
    "....kkkddkkk....",
    "...kggggggggk...",
    "...kkkkkkkkkk...",
    "................",
];

const DOCKER: [&str; 16] = [
    "................",
    "................",
    "..........kk....",
    ".....kk..kbbk...",
    "..kk.kbk.kbbk...",
    ".kbbkkbbkkbbkk..",
    ".kbbkkbbkkbbkbk.",
    ".kkkkkkkkkkkkbk.",
    "kbbbbbbbbbbbbbbk",
    "kbwbbbbbbbbbbbbk",
    ".kbbbbbbbbbbbbk.",
    "..kbbbbbbbbbbk..",
    "...kkbbbbbbkk...",
    ".....kkkkkk.....",
    "................",
    "................",
];

const NT4: [&str; 16] = [
    "................",
    "...kkkkkkkkkk...",
    "..kwwwwwwwwwwk..",
    "..kwggggggggdk..",
    "..kwkkkkkkkkdk..",
    "..kwggggggggdk..",
    "..kwkkkkkkkkdk..",
    "..kwggggggggdk..",
    "..kwkkkkkkkkdk..",
    "..kwggggggggdk..",
    "..kwggggGgggdk..",
    "..kwggggggggdk..",
    "..kwdddddddddk..",
    "...kkkkkkkkkk...",
    "..kkkkkkkkkkkk..",
    "................",
];

const WEB: [&str; 16] = [
    "................",
    "....kkkkkkkk....",
    "..kkbbbbbbbbkk..",
    ".kbbbGGbbbbbbbk.",
    ".kbbGGGGbbbGGbk.",
    "kbbbbGGGbbbGGGbk",
    "kbbbbbGGbbbbGbbk",
    "kbbbbbbbbbbbbbbk",
    "kbbbbGGbbbbbbbbk",
    "kbbbGGGGbbbbbbbk",
    ".kbbbGGGbbbbbbk.",
    ".kbbbbGGbbbbwbk.",
    "..kkbbbbbbbbkk..",
    "....kkkkkkkk....",
    "................",
    "................",
];

const COMMANDS: [&str; 16] = [
    "................",
    "................",
    ".kkkkkkkkkkkkkk.",
    ".knnnnnnnnnnwgk.",
    ".kwwwwwwwwwwwwk.",
    ".kkkkkkkkkkkkkk.",
    ".kkGkkkkkkkkkkk.",
    ".kkkGkkkkkkkkkk.",
    ".kkGkGGGGkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    ".kkkkkkkkkkkkkk.",
    "................",
];

const CONSOLE: [&str; 16] = [
    "................",
    "................",
    "..kkkkkkkkkkkk..",
    "..kwwwwwwwwwwk..",
    "..kwkkkkkkkkwk..",
    "..kwkGGkkGGkwk..",
    "..kwkkkkkkkkwk..",
    "..kwkGGGGkkkwk..",
    "..kwkkkkkkkkwk..",
    "..kwkGGkGGGkwk..",
    "..kwkkkkkkkkwk..",
    "..kwwwwwwwwwwk..",
    "..kkkkkkkkkkkk..",
    "................",
    "................",
    "................",
];

const HOSTS: [&str; 16] = [
    "................",
    "..kkkkkkkk......",
    ".kwwwwwwwwk.....",
    ".kwggggggdk.....",
    ".kwgttttgdk.....",
    ".kwgttttgdk.....",
    ".kwggggggdk.....",
    ".kkkkkkkkkk.....",
    "..kdddddk.kkkkk.",
    "..kkkkkkk.kwwwwk",
    "..........kwGgdk",
    "..........kwgwdk",
    "..........kkkkkk",
    "................",
    "................",
    "................",
];

const DNS: [&str; 16] = [
    "................",
    "................",
    "....kkkkkkkk....",
    "..kkbbbbbbbbkk..",
    ".kbbbbkGGkbbbbk.",
    ".kbbbkGGGGkbbbk.",
    "kbbbbbbGGbbbbbbk",
    "kbbbkkkkkkkkbbbk",
    "kbbbbbbbbbbbbbbk",
    ".kbbbbbkkbbbbbk.",
    ".kbbbbkddkbbbbk.",
    "..kkbbbkkbbbkk..",
    "....kkkkkkkk....",
    "................",
    "................",
    "................",
];

/// the icon for a module id, or for the built-in windows ("console", "hosts").
pub fn art(id: &str) -> &'static [&'static str; 16] {
    match id {
        "gateway" => &GATEWAY,
        "docker" => &DOCKER,
        "nt4-domain" => &NT4,
        "web-proxy" => &WEB,
        "remote-commands" => &COMMANDS,
        "dns" => &DNS,
        "console" => &CONSOLE,
        _ => &HOSTS,
    }
}

/// draw `art` with its top-left corner at `pos`, each pixel `scale` points wide.
pub fn draw(painter: &Painter, pos: Pos2, art: &[&str; 16], scale: f32) {
    for (y, row) in art.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            if let Some(colour) = colour(c) {
                let min = pos + Vec2::new(x as f32, y as f32) * scale;
                painter.rect_filled(Rect::from_min_size(min, Vec2::splat(scale)), 0.0, colour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_16_by_16_and_uses_known_colours() {
        for id in [
            "gateway",
            "docker",
            "nt4-domain",
            "web-proxy",
            "remote-commands",
            "dns",
            "console",
            "hosts",
        ] {
            let icon = art(id);
            for row in icon {
                assert_eq!(row.chars().count(), 16, "{id}: {row}");
                assert!(
                    row.chars().all(|c| c == '.' || colour(c).is_some()),
                    "{id}: {row}"
                );
            }
        }
    }
}
