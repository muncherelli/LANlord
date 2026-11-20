//! window geometry as pure functions on rects. the wm applies input to a rect before layout, so
//! nothing here reads egui state: the same inputs always give the same rect, and a test needs no
//! context.

use eframe::egui::{Pos2, Rect, Vec2};

/// how far apart cascaded windows sit; the same step the desktop uses today
pub const CASCADE_STEP: f32 = 22.0;

/// the kinds of caption button a frame can hit-test
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Close,
    Minimize,
    Maximize,
}

/// the part of a window frame under the pointer. the edges and corners are the resize handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Caption,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Button(ButtonKind),
    Content,
    None,
}

impl Section {
    /// whether a drag here moves the left edge
    pub fn left(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }

    /// whether a drag here moves the right edge
    pub fn right(self) -> bool {
        matches!(self, Self::Right | Self::TopRight | Self::BottomRight)
    }

    /// whether a drag here moves the top edge
    pub fn top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }

    /// whether a drag here moves the bottom edge
    pub fn bottom(self) -> bool {
        matches!(self, Self::Bottom | Self::BottomLeft | Self::BottomRight)
    }
}

/// clamp that tolerates an inverted range (a max below the min) by letting the min win, so a
/// bad size limit never panics mid-drag.
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi.max(lo)).max(lo)
}

/// the rect after dragging `s` by `delta` from `start`. only the dragged edges move, and each is
/// clamped against the opposite edge, which stays fixed. that is why a left or top drag can
/// never make the window walk across the desktop. `max` may be infinite.
pub fn resize(start: Rect, delta: Vec2, s: Section, min: Vec2, max: Vec2) -> Rect {
    let mut r = start;
    if s.left() {
        r.min.x = clamp(
            start.min.x + delta.x,
            start.max.x - max.x,
            start.max.x - min.x,
        );
    }
    if s.right() {
        r.max.x = clamp(
            start.max.x + delta.x,
            start.min.x + min.x,
            start.min.x + max.x,
        );
    }
    if s.top() {
        r.min.y = clamp(
            start.min.y + delta.y,
            start.max.y - max.y,
            start.max.y - min.y,
        );
    }
    if s.bottom() {
        r.max.y = clamp(
            start.max.y + delta.y,
            start.min.y + min.y,
            start.min.y + max.y,
        );
    }
    r
}

/// the rect after a caption drag: the size never changes
pub fn move_by(start: Rect, delta: Vec2) -> Rect {
    start.translate(delta)
}

/// move `rect` so it stays reachable in `area`, the desktop without the taskbar or menu bar.
/// the top edge never rises above the area, so the caption stays on screen, and at least
/// `keep_visible` points of the window stay inside on each axis. a `keep_visible` of the whole
/// window or more (use `f32::INFINITY`) keeps all of it inside, which is what the egui window
/// constrain does today; a window bigger than the area then sits at its top left corner.
/// the size is never changed.
pub fn clamp_to_work_area(rect: Rect, area: Rect, keep_visible: f32) -> Rect {
    let size = rect.size();
    let keep = Vec2::new(keep_visible.min(size.x), keep_visible.min(size.y));
    let x = clamp(
        rect.min.x,
        area.min.x - (size.x - keep.x),
        area.max.x - keep.x,
    );
    let y = clamp(rect.min.y, area.min.y, area.max.y - keep.y);
    Rect::from_min_size(Pos2::new(x, y), size)
}

/// the top left of the `index`th new window: each one `step` lower and further right than the
/// last, the first one a step from the area's corner. the count starts again once a window
/// would sit past a quarter of the area's width or height from the corner, so windows never
/// march off the desktop. the caller passes an area that already starts right of the icon column.
pub fn cascade(index: u32, area: Rect, step: f32) -> Pos2 {
    let room = area.width().min(area.height()) / 4.0;
    // the number of steps that fit in a quarter; at least one so the slot is always defined
    let slots = ((room / step).floor() as u32).max(1);
    area.min + Vec2::splat(((index % slots) + 1) as f32 * step)
}

/// round a rect's edges to whole physical pixels at `pixels_per_point`, so borders and bevels
/// are drawn on pixel boundaries instead of blurring across two. rounding twice changes nothing.
pub fn snap(rect: Rect, pixels_per_point: f32) -> Rect {
    let round = |v: f32| (v * pixels_per_point).round() / pixels_per_point;
    Rect::from_min_max(
        Pos2::new(round(rect.min.x), round(rect.min.y)),
        Pos2::new(round(rect.max.x), round(rect.max.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDGES: [Section; 8] = [
        Section::Left,
        Section::Right,
        Section::Top,
        Section::Bottom,
        Section::TopLeft,
        Section::TopRight,
        Section::BottomLeft,
        Section::BottomRight,
    ];

    fn start() -> Rect {
        Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(500.0, 380.0))
    }

    fn deltas() -> Vec<f32> {
        (-80..=80).map(|i| i as f32 * 10.0).collect()
    }

    #[test]
    fn the_helpers_name_the_edges_each_section_moves() {
        let table = [
            (Section::Left, [true, false, false, false]),
            (Section::Right, [false, true, false, false]),
            (Section::Top, [false, false, true, false]),
            (Section::Bottom, [false, false, false, true]),
            (Section::TopLeft, [true, false, true, false]),
            (Section::TopRight, [false, true, true, false]),
            (Section::BottomLeft, [true, false, false, true]),
            (Section::BottomRight, [false, true, false, true]),
            (Section::Caption, [false; 4]),
            (Section::Button(ButtonKind::Close), [false; 4]),
            (Section::Content, [false; 4]),
            (Section::None, [false; 4]),
        ];
        for (s, [l, r, t, b]) in table {
            assert_eq!(
                [s.left(), s.right(), s.top(), s.bottom()],
                [l, r, t, b],
                "{s:?}"
            );
        }
    }

    #[test]
    fn a_left_drag_never_changes_the_right_edge() {
        let min = Vec2::new(120.0, 90.0);
        for max in [Vec2::new(900.0, 700.0), Vec2::INFINITY] {
            for s in [Section::Left, Section::TopLeft, Section::BottomLeft] {
                for dx in deltas() {
                    let r = resize(start(), Vec2::new(dx, 7.0), s, min, max);
                    assert_eq!(r.max.x, start().max.x, "{s:?} dx {dx}");
                    assert!(r.width() >= min.x && r.width() <= max.x);
                }
            }
        }
    }

    #[test]
    fn a_top_drag_never_changes_the_bottom_edge() {
        let min = Vec2::new(120.0, 90.0);
        for max in [Vec2::new(900.0, 700.0), Vec2::INFINITY] {
            for s in [Section::Top, Section::TopLeft, Section::TopRight] {
                for dy in deltas() {
                    let r = resize(start(), Vec2::new(7.0, dy), s, min, max);
                    assert_eq!(r.max.y, start().max.y, "{s:?} dy {dy}");
                    assert!(r.height() >= min.y && r.height() <= max.y);
                }
            }
        }
    }

    #[test]
    fn a_bottom_right_drag_leaves_the_top_left_corner_alone() {
        let (min, max) = (Vec2::new(120.0, 90.0), Vec2::INFINITY);
        for dx in deltas() {
            let r = resize(start(), Vec2::new(dx, dx), Section::BottomRight, min, max);
            assert_eq!(r.min, start().min, "dx {dx}");
        }
    }

    #[test]
    fn shrinking_past_the_minimum_stops_at_the_minimum_with_the_far_edge_fixed() {
        let min = Vec2::new(120.0, 90.0);
        let max = Vec2::INFINITY;
        let far = Vec2::splat(5000.0);
        let r = resize(start(), far, Section::Left, min, max);
        assert_eq!((r.min.x, r.max.x), (start().max.x - min.x, start().max.x));
        let r = resize(start(), far, Section::Top, min, max);
        assert_eq!((r.min.y, r.max.y), (start().max.y - min.y, start().max.y));
        let r = resize(start(), -far, Section::Right, min, max);
        assert_eq!((r.min.x, r.max.x), (start().min.x, start().min.x + min.x));
        let r = resize(start(), -far, Section::Bottom, min, max);
        assert_eq!((r.min.y, r.max.y), (start().min.y, start().min.y + min.y));
    }

    #[test]
    fn growing_past_the_maximum_stops_at_the_maximum() {
        let (min, max) = (Vec2::new(120.0, 90.0), Vec2::new(600.0, 500.0));
        let r = resize(start(), Vec2::splat(-5000.0), Section::TopLeft, min, max);
        assert_eq!(r.size(), max);
        assert_eq!(r.max, start().max);
    }

    #[test]
    fn a_section_that_is_not_an_edge_does_not_resize() {
        let (min, max) = (Vec2::splat(10.0), Vec2::INFINITY);
        for s in [Section::Caption, Section::Content, Section::None] {
            assert_eq!(resize(start(), Vec2::splat(33.0), s, min, max), start());
        }
    }

    #[test]
    fn every_drag_on_every_edge_stays_between_the_limits() {
        let (min, max) = (Vec2::new(120.0, 90.0), Vec2::new(700.0, 600.0));
        for s in EDGES {
            for dx in deltas() {
                for dy in [-900.0, -37.0, 0.0, 37.0, 900.0] {
                    let r = resize(start(), Vec2::new(dx, dy), s, min, max);
                    assert!(r.width() >= min.x && r.width() <= max.x, "{s:?} {dx} {dy}");
                    assert!(
                        r.height() >= min.y && r.height() <= max.y,
                        "{s:?} {dx} {dy}"
                    );
                }
            }
        }
    }

    #[test]
    fn moving_keeps_the_size() {
        for d in deltas() {
            let r = move_by(start(), Vec2::new(d, -d / 2.0));
            assert_eq!(r.size(), start().size());
        }
    }

    fn area() -> Rect {
        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 700.0))
    }

    #[test]
    fn no_rect_leaves_the_work_area_after_a_full_clamp() {
        for x in (-20..=20).map(|i| i as f32 * 100.0) {
            for y in (-20..=20).map(|i| i as f32 * 100.0) {
                for size in [Vec2::new(300.0, 200.0), Vec2::new(900.0, 650.0)] {
                    let r = clamp_to_work_area(
                        Rect::from_min_size(Pos2::new(x, y), size),
                        area(),
                        f32::INFINITY,
                    );
                    assert!(area().contains_rect(r), "{x},{y} {size:?} -> {r:?}");
                    assert_eq!(r.size(), size);
                }
            }
        }
    }

    #[test]
    fn a_window_bigger_than_the_area_sits_at_its_corner() {
        let r = Rect::from_min_size(Pos2::new(300.0, 300.0), Vec2::new(2000.0, 2000.0));
        let c = clamp_to_work_area(r, area(), f32::INFINITY);
        assert_eq!(c.min, area().min);
    }

    #[test]
    fn a_partial_clamp_keeps_the_caption_reachable() {
        for x in (-30..=30).map(|i| i as f32 * 100.0) {
            for y in (-30..=30).map(|i| i as f32 * 100.0) {
                let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(400.0, 300.0));
                let c = clamp_to_work_area(r, area(), 40.0);
                assert!(c.min.y >= area().min.y && c.min.y <= area().max.y - 40.0);
                let seen = c.intersect(area());
                assert!(seen.width() >= 40.0, "{x},{y} -> {c:?}");
                assert!(seen.height() >= 40.0, "{x},{y} -> {c:?}");
            }
        }
    }

    #[test]
    fn a_window_already_in_place_is_not_moved() {
        let r = Rect::from_min_size(Pos2::new(50.0, 60.0), Vec2::new(300.0, 200.0));
        assert_eq!(clamp_to_work_area(r, area(), f32::INFINITY), r);
        assert_eq!(clamp_to_work_area(r, area(), 40.0), r);
    }

    #[test]
    fn the_cascade_steps_then_wraps_at_a_quarter_of_the_area() {
        // 700 high: a quarter is 175, so 7 steps of 22 fit and the 8th wraps
        let a = area();
        let at = |i| cascade(i, a, CASCADE_STEP);
        assert_eq!(at(0), Pos2::new(22.0, 22.0));
        assert_eq!(at(1), Pos2::new(44.0, 44.0));
        assert_eq!(at(6), Pos2::new(154.0, 154.0));
        assert_eq!(at(7), at(0));
        assert_eq!(at(7 * 5 + 3), at(3));
    }

    #[test]
    fn the_cascade_follows_the_area_corner_and_never_leaves_a_quarter() {
        let a = Rect::from_min_size(Pos2::new(60.0, 10.0), Vec2::new(800.0, 400.0));
        for i in 0..100 {
            let p = cascade(i, a, CASCADE_STEP);
            assert!(p.x > a.min.x && p.y > a.min.y);
            assert!(p.x - a.min.x <= a.width() / 4.0 && p.y - a.min.y <= a.height() / 4.0);
        }
    }

    #[test]
    fn a_tiny_area_still_gives_a_slot() {
        let a = Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0));
        assert_eq!(cascade(5, a, CASCADE_STEP), Pos2::new(22.0, 22.0));
    }

    #[test]
    fn snap_is_idempotent_and_lands_on_whole_pixels() {
        for ppp in [1.0, 1.25, 1.5, 2.0, 3.0] {
            for i in 0..200 {
                let v = i as f32 * 0.37 - 20.0;
                let r = Rect::from_min_max(Pos2::new(v, v * 1.3), Pos2::new(v + 50.3, v + 91.7));
                let once = snap(r, ppp);
                assert_eq!(snap(once, ppp), once, "ppp {ppp} v {v}");
                for e in [once.min.x, once.min.y, once.max.x, once.max.y] {
                    assert!(
                        (e * ppp - (e * ppp).round()).abs() < 1e-3,
                        "ppp {ppp} e {e}"
                    );
                }
            }
        }
    }
}
