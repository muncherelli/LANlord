//! the colours the console window draws lanlord's log lines in; the window itself lives in
//! `lanlord_app::console`.

use eframe::egui::Color32;
use lanlord_app::log::Kind;

/// the colour a line is shown in on the black console.
pub fn colour(kind: Kind) -> Color32 {
    match kind {
        Kind::Out => Color32::LIGHT_GRAY,
        Kind::Err => Color32::from_rgb(255, 85, 85),
        Kind::Info => Color32::from_rgb(85, 255, 255),
        Kind::Debug => Color32::from_rgb(128, 128, 128),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_its_own_colour() {
        let all = [Kind::Out, Kind::Err, Kind::Info, Kind::Debug].map(colour);
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| a != b));
        }
    }
}
