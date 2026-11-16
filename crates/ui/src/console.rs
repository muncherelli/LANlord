//! what lanlord prints, collected from `lanlord_app::log` for the console window.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
use std::sync::mpsc::Receiver;

use eframe::egui::{Color32, Context};
use lanlord_app::log::{self, Kind, Line};

/// the log's lines, with a repaint for each on a desktop; the browser has no threads, so there
/// the lines are read each frame.
#[cfg(not(target_arch = "wasm32"))]
fn forward(ctx: &Context) -> Receiver<Line> {
    let (forward, incoming) = mpsc::channel();
    let from_log = log::subscribe();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        for line in from_log {
            if forward.send(line).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    incoming
}

#[cfg(target_arch = "wasm32")]
fn forward(_ctx: &Context) -> Receiver<Line> {
    log::subscribe()
}

/// how many lines to keep.
const KEEP: usize = 2000;

pub struct Console {
    lines: Vec<Line>,
    incoming: Receiver<Line>,
}

impl Console {
    /// subscribe to the log; the ui is woken whenever a line arrives.
    pub fn new(ctx: &Context) -> Self {
        Self {
            lines: Vec::new(),
            incoming: forward(ctx),
        }
    }

    /// move what arrived since the last frame into the list.
    pub fn drain(&mut self) {
        self.lines.extend(self.incoming.try_iter());
        if self.lines.len() > KEEP {
            self.lines.drain(..self.lines.len() - KEEP);
        }
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }
}

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
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn lines_arrive_in_order_and_old_ones_fall_off() {
        let mut console = Console::new(&Context::default());
        for n in 0..KEEP + 5 {
            log::emit(Kind::Info, format!("console-test-{n}"));
        }
        let start = Instant::now();
        loop {
            console.drain();
            let mine = console
                .lines()
                .iter()
                .filter(|l| l.text.starts_with("console-test-"))
                .count();
            if mine >= KEEP || start.elapsed() > Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(console.lines().len(), KEEP);
        let last = console.lines().last().unwrap();
        assert_eq!(last.text, format!("console-test-{}", KEEP + 4));
    }

    #[test]
    fn every_kind_has_its_own_colour() {
        let all = [Kind::Out, Kind::Err, Kind::Info, Kind::Debug].map(colour);
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| a != b));
        }
    }
}
