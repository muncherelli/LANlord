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

/// clamp that tolerates an inverted range (`hi` below `lo`) by letting `lo` win
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi).max(lo)
}

/// the rect after dragging `s` by `delta` from `start`. only the dragged edges move, and each is
/// clamped against the opposite edge, which stays fixed. that is why a left or top drag can
/// never make the window walk across the desktop. each dragged edge also stops at `bounds` (the
/// work area), so a drag cannot push the caption off screen; moving the whole rect afterwards
/// could not do that without moving the fixed edge. `max` may be infinite. when the limits
/// disagree (`max` below `min`, or less room in `bounds` than `min`) the minimum size wins.
pub fn resize(start: Rect, delta: Vec2, s: Section, min: Vec2, max: Vec2, bounds: Rect) -> Rect {
    let max = max.max(min);
    // a moving min edge: the smallest size is the upper limit and wins over size and bounds
    let near = |v: f32, far: f32, min: f32, max: f32, bound: f32| {
        v.max((far - max).max(bound)).min(far - min)
    };
    // a moving max edge: the smallest size is the lower limit and wins over size and bounds
    let far = |v: f32, near: f32, min: f32, max: f32, bound: f32| {
        clamp(v, near + min, (near + max).min(bound))
    };
    let mut r = start;
    if s.left() {
        r.min.x = near(
            start.min.x + delta.x,
            start.max.x,
            min.x,
            max.x,
            bounds.min.x,
        );
    }
    if s.right() {
        r.max.x = far(
            start.max.x + delta.x,
            start.min.x,
            min.x,
            max.x,
            bounds.max.x,
        );
    }
    if s.top() {
        r.min.y = near(
            start.min.y + delta.y,
            start.max.y,
            min.y,
            max.y,
            bounds.min.y,
        );
    }
    if s.bottom() {
        r.max.y = far(
            start.max.y + delta.y,
            start.min.y,
            min.y,
            max.y,
            bounds.max.y,
        );
    }
    r
}

/// a resize or move in progress. everything is frozen at press time and the rect is always
/// recomputed from `start` and the pointer's total travel, so the drag has no accumulated
/// error and the limits cannot change mid-drag. PR 4 deletes `CASCADE_STEP` and `take_slot`
/// in desktop.rs in favour of this module.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub section: Section,
    pub start: Rect,
    pub pointer_start: Pos2,
    pub min: Vec2,
    pub max: Vec2,
}

impl Drag {
    /// the window's rect with the pointer at `pointer`: a caption drag moves it and keeps it
    /// inside `bounds` (as the egui constrain does today), an edge drag resizes it, anything
    /// else leaves it alone.
    pub fn apply(&self, pointer: Pos2, bounds: Rect) -> Rect {
        let delta = pointer - self.pointer_start;
        match self.section {
            Section::Caption => {
                clamp_to_work_area(move_by(self.start, delta), bounds, f32::INFINITY)
            }
            s => resize(self.start, delta, s, self.min, self.max, bounds),
        }
    }
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
/// last, the first one a step from `area`'s corner plus `corner` (the desktop's icon column).
/// the count starts again once a window would sit a quarter of the full desktop's width or height
/// from the corner, so windows never march off the desktop. the quarter is measured on `area`
/// itself, not on what is left after `corner`, as the desktop does today.
pub fn cascade(index: u32, area: Rect, corner: Vec2, step: f32) -> Pos2 {
    let room = area.width().min(area.height()) / 4.0;
    // the number of steps that fit in a quarter; at least one so the slot is always defined
    let slots = ((room / step).floor() as u32).max(1);
    area.min + corner + Vec2::splat(((index % slots) + 1) as f32 * step)
}

/// round a rect to whole physical pixels at `pixels_per_point`, so borders and bevels are drawn
/// on pixel boundaries instead of blurring across two. the corner and the size are rounded on
/// their own, not the two edges, so a window's size never jitters by a pixel as it moves.
/// rounding twice changes nothing.
pub fn snap(rect: Rect, pixels_per_point: f32) -> Rect {
    let round = |v: f32| (v * pixels_per_point).round() / pixels_per_point;
    let size = rect.size();
    Rect::from_min_size(
        Pos2::new(round(rect.min.x), round(rect.min.y)),
        Vec2::new(round(size.x), round(size.y)),
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

    fn open() -> Rect {
        Rect::from_min_max(Pos2::new(-1e6, -1e6), Pos2::new(1e6, 1e6))
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
                    let r = resize(start(), Vec2::new(dx, 7.0), s, min, max, open());
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
                    let r = resize(start(), Vec2::new(7.0, dy), s, min, max, open());
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
            let r = resize(
                start(),
                Vec2::new(dx, dx),
                Section::BottomRight,
                min,
                max,
                open(),
            );
            assert_eq!(r.min, start().min, "dx {dx}");
        }
    }

    #[test]
    fn shrinking_past_the_minimum_stops_at_the_minimum_with_the_far_edge_fixed() {
        let min = Vec2::new(120.0, 90.0);
        let max = Vec2::INFINITY;
        let far = Vec2::splat(5000.0);
        let r = resize(start(), far, Section::Left, min, max, open());
        assert_eq!((r.min.x, r.max.x), (start().max.x - min.x, start().max.x));
        let r = resize(start(), far, Section::Top, min, max, open());
        assert_eq!((r.min.y, r.max.y), (start().max.y - min.y, start().max.y));
        let r = resize(start(), -far, Section::Right, min, max, open());
        assert_eq!((r.min.x, r.max.x), (start().min.x, start().min.x + min.x));
        let r = resize(start(), -far, Section::Bottom, min, max, open());
        assert_eq!((r.min.y, r.max.y), (start().min.y, start().min.y + min.y));
    }

    #[test]
    fn growing_past_the_maximum_stops_at_the_maximum() {
        let (min, max) = (Vec2::new(120.0, 90.0), Vec2::new(600.0, 500.0));
        let r = resize(
            start(),
            Vec2::splat(-5000.0),
            Section::TopLeft,
            min,
            max,
            open(),
        );
        assert_eq!(r.size(), max);
        assert_eq!(r.max, start().max);
    }

    #[test]
    fn a_section_that_is_not_an_edge_does_not_resize() {
        let (min, max) = (Vec2::splat(10.0), Vec2::INFINITY);
        for s in [Section::Caption, Section::Content, Section::None] {
            assert_eq!(
                resize(start(), Vec2::splat(33.0), s, min, max, open()),
                start()
            );
        }
    }

    #[test]
    fn every_drag_on_every_edge_stays_between_the_limits() {
        let (min, max) = (Vec2::new(120.0, 90.0), Vec2::new(700.0, 600.0));
        for s in EDGES {
            for dx in deltas() {
                for dy in [-900.0, -37.0, 0.0, 37.0, 900.0] {
                    let r = resize(start(), Vec2::new(dx, dy), s, min, max, open());
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

    const ICON_COLUMN: f32 = 80.0;

    fn slot(i: u32, a: Rect) -> Pos2 {
        cascade(i, a, Vec2::new(ICON_COLUMN, 0.0), CASCADE_STEP)
    }

    #[test]
    fn new_windows_step_22_pixels_and_start_again_at_a_quarter_of_the_desktop() {
        // the same case as the desktop's take_slot test: a step passes a quarter (152.5) after 132
        let a = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 610.0));
        let at = |n: f32| Pos2::new(ICON_COLUMN + 22.0 * n, 22.0 * n);
        let slots: Vec<Pos2> = (0..9).map(|i| slot(i, a)).collect();
        for (n, pos) in slots.iter().take(6).enumerate() {
            assert_eq!(*pos, at(n as f32 + 1.0), "{n}");
        }
        assert_eq!(slots[6], at(1.0), "the seventh wraps");
        assert_eq!(slots[7], at(2.0));
        // a wide desktop wraps on its height, measured from its own corner
        let wide = Rect::from_min_size(Pos2::new(5.0, 7.0), Vec2::new(4000.0, 20.0));
        let first = slot(0, wide);
        assert_eq!(first, Pos2::new(5.0 + ICON_COLUMN + 22.0, 7.0 + 22.0));
        assert_eq!(slot(1, wide), first, "22 is past 20 / 4");
    }

    #[test]
    fn the_cascade_wraps_for_ever_and_never_leaves_a_quarter() {
        let a = Rect::from_min_size(Pos2::new(60.0, 10.0), Vec2::new(800.0, 400.0));
        for i in 0..100 {
            let p = slot(i, a);
            assert!(p.x > a.min.x + ICON_COLUMN && p.y > a.min.y);
            assert!(p.y - a.min.y <= a.height() / 4.0);
        }
        assert_eq!(slot(0, a), slot(4, a), "100 high steps 4 times");
    }

    #[test]
    fn an_inverted_size_range_lets_the_minimum_win_on_every_edge() {
        let (min, max) = (Vec2::new(200.0, 150.0), Vec2::new(100.0, 50.0));
        for s in EDGES {
            for d in [-2000.0, -50.0, 0.0, 50.0, 2000.0] {
                let r = resize(start(), Vec2::splat(d), s, min, max, open());
                if s.left() || s.right() {
                    assert!(r.width() >= min.x, "{s:?} {d}: {r:?}");
                }
                if s.top() || s.bottom() {
                    assert!(r.height() >= min.y, "{s:?} {d}: {r:?}");
                }
            }
        }
        // a hard push to grow gives the minimum, the same from either side
        let l = resize(
            start(),
            Vec2::splat(-2000.0),
            Section::TopLeft,
            min,
            max,
            open(),
        );
        let r = resize(
            start(),
            Vec2::splat(2000.0),
            Section::BottomRight,
            min,
            max,
            open(),
        );
        assert_eq!(l.size(), min);
        assert_eq!(r.size(), min);
    }

    #[test]
    fn a_drag_stops_at_the_work_area_with_the_far_edge_fixed() {
        let b = area();
        let inside = Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(500.0, 380.0));
        let (min, max) = (Vec2::new(120.0, 90.0), Vec2::INFINITY);
        for dx in deltas() {
            let l = resize(inside, Vec2::splat(dx), Section::TopLeft, min, max, b);
            assert!(l.min.x >= b.min.x && l.min.y >= b.min.y, "{dx}: {l:?}");
            assert_eq!(l.max, inside.max);
            let r = resize(inside, Vec2::splat(dx), Section::BottomRight, min, max, b);
            assert!(r.max.x <= b.max.x && r.max.y <= b.max.y, "{dx}: {r:?}");
            assert_eq!(r.min, inside.min);
        }
        let l = resize(inside, Vec2::splat(-5000.0), Section::TopLeft, min, max, b);
        assert_eq!(l.min, b.min);
    }

    #[test]
    fn the_minimum_wins_when_the_work_area_has_less_room() {
        let b = Rect::from_min_max(Pos2::ZERO, Vec2::new(150.0, 150.0).to_pos2());
        let min = Vec2::splat(120.0);
        let inside = Rect::from_min_max(Pos2::new(60.0, 60.0), Pos2::new(140.0, 140.0));
        let r = resize(
            inside,
            Vec2::splat(500.0),
            Section::BottomRight,
            min,
            Vec2::INFINITY,
            b,
        );
        assert_eq!(r.size(), min);
        let l = resize(
            inside,
            Vec2::splat(-500.0),
            Section::TopLeft,
            min,
            Vec2::INFINITY,
            b,
        );
        assert_eq!(l.max, inside.max);
        assert!(l.width() >= min.x);
    }

    #[test]
    fn a_drag_freezes_its_limits_and_follows_the_pointer_from_the_press() {
        let d = Drag {
            section: Section::Left,
            start: start(),
            pointer_start: Pos2::new(100.0, 200.0),
            min: Vec2::new(120.0, 90.0),
            max: Vec2::INFINITY,
        };
        let r = d.apply(Pos2::new(60.0, 999.0), open());
        assert_eq!(r, Rect::from_min_max(Pos2::new(60.0, 80.0), start().max));
        // the same pointer gives the same rect however the drag got there
        assert_eq!(d.apply(Pos2::new(60.0, 999.0), open()), r);
        let m = Drag {
            section: Section::Caption,
            ..d
        };
        let moved = m.apply(Pos2::new(130.0, 210.0), open());
        assert_eq!(moved, start().translate(Vec2::new(30.0, 10.0)));
        // a caption drag cannot leave the work area
        let out = m.apply(Pos2::new(-9000.0, -9000.0), area());
        assert!(area().contains_rect(out));
        assert_eq!(out.size(), start().size());
        let c = Drag {
            section: Section::Content,
            ..d
        };
        assert_eq!(c.apply(Pos2::new(5.0, 5.0), open()), start());
    }

    #[test]
    fn moving_never_changes_the_snapped_size_at_fractional_scales() {
        for ppp in [1.25, 1.5, 1.75, 2.5] {
            let base = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(50.3, 71.7));
            let size = snap(base, ppp).size();
            for i in 0..200 {
                let moved = move_by(base, Vec2::splat(i as f32 * 0.1));
                let got = snap(moved, ppp).size() * ppp;
                let got = (got.x.round(), got.y.round());
                assert_eq!(
                    got,
                    ((size.x * ppp).round(), (size.y * ppp).round()),
                    "ppp {ppp} i {i}"
                );
            }
        }
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
