//! the window host's look and hit zones, hard-coded to NT4 in one place until the look supplies
//! them. the windows are still `egui::Window`s; the wm takes over their rects in a later change.

use eframe::egui::{Frame, Margin, Rect};

use crate::theme::GRAY;
use crate::windows::FRAME;

/// the frame of a window: gray, with a border the contents keep clear of.
pub(crate) fn frame() -> Frame {
    Frame::new()
        .fill(GRAY)
        .inner_margin(Margin::same(FRAME as i8))
}

/// which of a window's edges a press at `p` takes hold of, as `(left, right, top, bottom)`: a few
/// pixels (`side`) either side of an edge, and a larger square (`corner`) around a corner.
pub(crate) fn grab(
    side: f32,
    corner: f32,
    rect: Rect,
    p: eframe::egui::Pos2,
) -> (bool, bool, bool, bool) {
    let near = |a: f32, b: f32, r: f32| (a - b).abs() <= r;
    let mut zones = [
        near(p.x, rect.left(), side),
        near(p.x, rect.right(), side),
        near(p.y, rect.top(), side),
        near(p.y, rect.bottom(), side),
    ];
    // a corner takes both its edges
    for (x, y, h, v) in [
        (rect.left(), rect.top(), 0, 2),
        (rect.right(), rect.top(), 1, 2),
        (rect.left(), rect.bottom(), 0, 3),
        (rect.right(), rect.bottom(), 1, 3),
    ] {
        if near(p.x, x, corner) && near(p.y, y, corner) {
            zones[h] = true;
            zones[v] = true;
        }
    }
    (zones[0], zones[1], zones[2], zones[3])
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Pos2, Vec2};

    use super::*;

    fn window() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(300.0, 200.0))
    }

    #[test]
    fn an_edge_is_grabbed_a_few_pixels_either_side() {
        let (l, r, t, b) = grab(5.0, 10.0, window(), Pos2::new(97.0, 200.0));
        assert_eq!((l, r, t, b), (true, false, false, false));
        let (l, r, t, b) = grab(5.0, 10.0, window(), Pos2::new(250.0, 304.0));
        assert_eq!((l, r, t, b), (false, false, false, true));
        let none = grab(5.0, 10.0, window(), Pos2::new(250.0, 200.0));
        assert_eq!(none, (false, false, false, false));
    }

    #[test]
    fn a_corner_takes_both_its_edges_in_a_wider_square() {
        let at = Pos2::new(400.0 + 8.0, 300.0 - 8.0);
        assert_eq!(grab(5.0, 10.0, window(), at), (false, true, false, true));
    }

    #[test]
    fn the_frame_is_gray_with_the_border_as_margin() {
        let f = frame();
        assert_eq!(f.fill, GRAY);
        assert_eq!(f.inner_margin, Margin::same(FRAME as i8));
    }
}
