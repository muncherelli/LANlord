//! draws a `Tree` of the layout tree with egui: a white box of rows with small icons, dotted
//! guide lines and a box with a plus or a minus before each branch, like the left pane of the
//! NT4 management console. clicks send `select` or `open`; with the keyboard on it, Up and Down
//! move the selection, Left and Right close, open and walk, Space, Plus and Minus open and close.

use eframe::egui::{self, Key, Modifiers, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, Vec2};
use lanlord_app::view::{Tree, TreeKey, TreeRow};

use crate::controls;
use crate::icons;
use crate::theme::{self, BLACK, NAVY, SHADOW, WHITE};

const ROW: f32 = 18.0;
/// how far each level is indented.
const STEP: f32 = 16.0;
const BOX: f32 = 9.0;

/// does the row at `i` have a later row at its own depth under the same parent?
fn has_next(rows: &[TreeRow], i: usize) -> bool {
    rows[i + 1..]
        .iter()
        .find(|r| r.depth <= rows[i].depth)
        .is_some_and(|r| r.depth == rows[i].depth)
}

/// for each level left of the row at `i`, does a guide line run down through it? the last of
/// them is the line the row hangs from.
fn guides(rows: &[TreeRow], i: usize) -> Vec<bool> {
    (1..=rows[i].depth)
        .map(|level| {
            rows[..=i]
                .iter()
                .rposition(|r| r.depth == level)
                .is_some_and(|at| has_next(rows, at))
        })
        .collect()
}

/// a dotted line, every other point.
fn dots(ui: &Ui, from: Pos2, to: Pos2) {
    let along = to - from;
    let steps = (along.x.abs() + along.y.abs()) as i32;
    for n in (0..=steps).step_by(2) {
        let at = from + along * (n as f32 / steps.max(1) as f32);
        let at = at.round();
        ui.painter()
            .rect_filled(Rect::from_min_size(at, Vec2::splat(1.0)), 0.0, SHADOW);
    }
}

fn sign_box(ui: &Ui, rect: Rect, open: bool) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, WHITE);
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, SHADOW),
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    painter.line_segment(
        [c - Vec2::new(2.0, 0.0), c + Vec2::new(2.0, 0.0)],
        Stroke::new(1.0, BLACK),
    );
    if !open {
        painter.line_segment(
            [c - Vec2::new(0.0, 2.0), c + Vec2::new(0.0, 2.0)],
            Stroke::new(1.0, BLACK),
        );
    }
}

/// where the sign box of a row at this depth sits in the row.
fn sign_rect(row: Rect, depth: u8) -> Rect {
    let x = row.left() + 4.0 + f32::from(depth) * STEP;
    Rect::from_center_size(Pos2::new(x + BOX / 2.0, row.center().y), Vec2::splat(BOX))
}

/// draw one row: guides, sign box, icon and label.
fn row_ui(ui: &Ui, tree: &Tree<impl Clone>, i: usize, rect: Rect) {
    let row = &tree.rows[i];
    let selected = tree.selected == Some(i);
    let mid = rect.center().y;
    // a guide of level `l` runs at the middle of the sign box of depth `l - 1`
    let guide_x = |level: usize| rect.left() + 4.0 + (level as f32 - 1.0) * STEP + BOX / 2.0;
    let lines = guides(&tree.rows, i);
    for (n, goes_on) in lines.iter().enumerate() {
        let level = n + 1;
        let x = guide_x(level);
        if level == lines.len() {
            dots(
                ui,
                Pos2::new(x, rect.top()),
                Pos2::new(x, if *goes_on { rect.bottom() } else { mid }),
            );
            let end = x + STEP
                - BOX / 2.0
                - if row.open.is_some() {
                    BOX / 2.0 + 1.0
                } else {
                    0.0
                };
            dots(ui, Pos2::new(x, mid), Pos2::new(end.max(x), mid));
        } else if *goes_on {
            dots(ui, Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()));
        }
    }
    if let Some(open) = row.open {
        sign_box(ui, sign_rect(rect, row.depth), open);
    }
    let mut x = rect.left() + 4.0 + f32::from(row.depth) * STEP + BOX + 4.0;
    if let Some(icon) = row.icon {
        icons::draw_small(ui.painter(), Pos2::new(x, mid - 8.0), icon);
        x += 18.0;
    }
    let ink = if selected { WHITE } else { BLACK };
    let galley = ui
        .painter()
        .layout_no_wrap(row.label.clone(), theme::font(), ink);
    let at = Pos2::new(x, mid - galley.size().y / 2.0);
    if selected {
        let back = Rect::from_min_size(
            at - Vec2::new(2.0, 1.0),
            galley.size() + Vec2::new(4.0, 2.0),
        );
        ui.painter().rect_filled(back.intersect(rect), 0.0, NAVY);
    }
    ui.painter().galley(at, galley, ink);
}

/// the keys that move around the tree while it has the keyboard.
fn keys<M>(ui: &Ui, tree: &Tree<M>) -> Option<M> {
    let pressed = |key: Key| ui.input_mut(|i| i.consume_key(Modifiers::NONE, key));
    let at = tree.selected?;
    let row = tree.rows.get(at)?;
    let go = |to: usize| (to < tree.rows.len()).then(|| (tree.select)(to));
    let set = |open: bool| {
        row.open
            .filter(|now| *now != open)
            .map(|_| (tree.open)(at, open))
    };
    if pressed(Key::ArrowDown) {
        go(at + 1)
    } else if pressed(Key::ArrowUp) {
        at.checked_sub(1).and_then(go)
    } else if pressed(Key::ArrowLeft) {
        tree.key(TreeKey::Left)
    } else if pressed(Key::ArrowRight) {
        tree.key(TreeKey::Right)
    } else if pressed(Key::Plus) || pressed(Key::Equals) {
        set(true)
    } else if pressed(Key::Minus) {
        set(false)
    } else if pressed(Key::Space) {
        row.open.map(|now| (tree.open)(at, !now))
    } else {
        None
    }
}

/// the tree in a white box that scrolls; the message of a click or a key.
pub fn tree<M: Clone>(ui: &mut Ui, tree: &Tree<M>) -> Option<M> {
    let mut sent = None;
    let id = ui.scope_id().with(tree.id);
    let height = controls::grow(ui, tree.height);
    let mut clicked_in = false;
    let shown = theme::sunken(ui, WHITE, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(height);
        ScrollArea::vertical()
            .id_salt(tree.id)
            .max_height(height)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, row) in tree.rows.iter().enumerate() {
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                    if ui.is_rect_visible(rect) {
                        row_ui(ui, tree, i, rect);
                    }
                    if response.clicked() {
                        clicked_in = true;
                        let on_box = row.open.is_some()
                            && response.interact_pointer_pos().is_some_and(|p| {
                                sign_rect(rect, row.depth).expand(3.0).contains(p)
                            });
                        sent = Some(match row.open {
                            Some(open) if on_box => (tree.open)(i, !open),
                            _ => (tree.select)(i),
                        });
                    }
                }
                ui.min_rect()
            })
            .inner
    });
    let focus = ui.interact(shown, id, Sense::focusable_noninteractive());
    if clicked_in {
        focus.request_focus();
    }
    if focus.has_focus() {
        // the arrows are the tree's: otherwise egui moves the focus to the next widget
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
    }
    if focus.has_focus() && sent.is_none() {
        sent = keys(ui, tree);
    }
    sent
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use lanlord_app::view::Tree;

    use super::*;

    fn rows() -> Vec<TreeRow> {
        let row = |label: &str| TreeRow {
            depth: 0,
            label: label.into(),
            icon: None,
            open: None,
        };
        vec![row("one"), row("two"), row("three"), row("four")]
    }

    #[test]
    fn two_arrows_in_a_row_move_the_selection_twice() {
        let mut harness = Harness::new_ui_state(
            |ui, selected: &mut usize| {
                let tree = Tree {
                    id: "t",
                    rows: rows(),
                    selected: Some(*selected),
                    height: 100.0,
                    select: |n| n,
                    open: |n, _| n,
                };
                if let Some(to) = super::tree(ui, &tree) {
                    *selected = to;
                }
                // a widget the focus could wander to
                let _ = ui.button("elsewhere");
            },
            0,
        );
        harness.run();
        // a click on a row picks it and gives the tree the keyboard
        let pos = Pos2::new(40.0, 30.0);
        harness.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run();
        }
        harness.run();
        assert_eq!(*harness.state(), 1);
        harness.key_press(Key::ArrowDown);
        harness.run();
        harness.key_press(Key::ArrowDown);
        harness.run();
        assert_eq!(*harness.state(), 3);
        harness.key_press(Key::ArrowUp);
        harness.run();
        assert_eq!(*harness.state(), 2);
    }
}
