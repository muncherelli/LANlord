//! windows with a navy title bar and the three title-bar buttons.

use eframe::egui::{self, Align2, Id, LayerId, Margin, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::controls;
use crate::icons;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, SHADOW, WHITE};
use crate::view;

/// what the title-bar buttons asked for this frame.
#[derive(Default)]
pub struct TitleActions {
    pub close: bool,
    pub minimize: bool,
}

fn title_button(ui: &mut Ui, rect: Rect, id: Id, glyph: Glyph) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    let down = response.is_pointer_button_down_on();
    ui.painter().rect_filled(rect, 0.0, GRAY);
    bevel(ui.painter(), rect, !down);
    let c = rect.center()
        + if down {
            Vec2::new(1.0, 1.0)
        } else {
            Vec2::ZERO
        };
    let stroke = Stroke::new(1.5, BLACK);
    match glyph {
        Glyph::Minimize => {
            let y = c.y + 3.0;
            ui.painter()
                .line_segment([Pos2::new(c.x - 4.0, y), Pos2::new(c.x + 3.0, y)], stroke);
        }
        Glyph::Maximize => {
            ui.painter().rect_stroke(
                Rect::from_center_size(c, Vec2::splat(8.0)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Glyph::Close => {
            for (a, b) in [((-3.5, -3.5), (3.5, 3.5)), ((-3.5, 3.5), (3.5, -3.5))] {
                ui.painter()
                    .line_segment([c + Vec2::new(a.0, a.1), c + Vec2::new(b.0, b.1)], stroke);
            }
        }
    }
    response.clicked()
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Close,
}

fn title_bar(
    ui: &mut Ui,
    rect: Rect,
    icon: &str,
    title: &str,
    active: bool,
    dialog: bool,
    fixed: bool,
) -> TitleActions {
    ui.painter()
        .rect_filled(rect, 0.0, if active { NAVY } else { SHADOW });
    icons::draw_small(ui.painter(), rect.left_top() + Vec2::new(3.0, 2.0), icon);
    ui.painter().text(
        rect.left_center() + Vec2::new(TITLE_TEXT, 0.0),
        Align2::LEFT_CENTER,
        title,
        theme::font(),
        if active { WHITE } else { GRAY },
    );
    let size = Vec2::new(TITLE_BUTTON, 14.0);
    let top = rect.top() + 3.0;
    let close = Rect::from_min_size(Pos2::new(rect.right() - 2.0 - size.x, top), size);
    let base = ui.scope_id().with(title);
    // a dialog has only a close button
    if dialog {
        return TitleActions {
            close: title_button(ui, close, base.with("close"), Glyph::Close),
            minimize: false,
        };
    }
    // a fixed window has no maximize button, and its minimize sits beside close
    if fixed {
        let minimize = close.translate(Vec2::new(-size.x - 2.0, 0.0));
        return TitleActions {
            close: title_button(ui, close, base.with("close"), Glyph::Close),
            minimize: title_button(ui, minimize, base.with("min"), Glyph::Minimize),
        };
    }
    let maximize = close.translate(Vec2::new(-size.x - 2.0, 0.0));
    let minimize = maximize.translate(Vec2::new(-size.x, 0.0));
    let minimize = title_button(ui, minimize, base.with("min"), Glyph::Minimize);
    // the desktop has no use for maximize yet; it is drawn for the look
    title_button(ui, maximize, base.with("max"), Glyph::Maximize);
    TitleActions {
        close: title_button(ui, close, base.with("close"), Glyph::Close),
        minimize,
    }
}

/// the close button at the right end of a dialog's title bar; true when it was clicked.
pub(crate) fn dialog_close(ui: &mut Ui, bar: Rect, id: Id) -> bool {
    let size = Vec2::new(TITLE_BUTTON, 14.0);
    let close = Rect::from_min_size(Pos2::new(bar.right() - 2.0 - size.x, bar.top() + 3.0), size);
    title_button(ui, close, id, Glyph::Close)
}

/// where a window goes and how much room it has: the desktop above the taskbar.
#[derive(Clone, Copy)]
pub struct Placement {
    /// the window's top left for a document window, its middle for a dialog.
    pub start: Pos2,
    /// the width it opens at.
    pub width: f32,
    /// the desktop it has to stay inside.
    pub area: Rect,
    /// the contents can grow (a console, a list, a text): the window can be resized and the
    /// growable part of its contents takes the room.
    pub growable: bool,
    /// a fixed dialog like System Properties: minimize and close only, no resizing, and always
    /// `width` by `height`, which must be given.
    pub fixed: bool,
    /// the height a growable window opens at when it is not as tall as its contents, like a
    /// console of 24 lines. `None` opens it as tall as its contents, stopping at the desktop.
    pub height: Option<f32>,
    /// the least height a growable window opens at, for the contents that are near empty. a
    /// window with more than that is as tall as its contents.
    pub floor: f32,
    /// the id of the window's icon, which is not the window's own id.
    pub icon: &'static str,
    /// the window has just opened: it takes `start` as its position once, instead of the place
    /// egui remembers from the last time it was open.
    pub fresh: bool,
}

/// the layer a window's area lives on, to raise it with `Context::move_to_top`. it is the id the
/// window is given in `show`.
pub fn layer(id: &str) -> LayerId {
    LayerId::new(Order::Middle, Id::unique(id))
}

/// what a growable window's contents were, to share its room out the next frame.
#[derive(Clone, Copy)]
struct Room {
    /// how tall, with their margins, without room given to the parts that grow
    natural: f32,
    /// how tall the parts that grow were on their own
    claimed: f32,
    /// a part that never shrinks took room, so the contents are never smaller than they are
    whole: bool,
    /// the window has been given a size, which it keeps
    user: bool,
    /// frames the contents have been as tall as they are
    age: u8,
}

/// where a dialog is in being placed. it opens with its middle at the start, and a tabbed one,
/// once it has a size, is held by its top left instead, so that a different page cannot move it.
#[derive(Clone, Copy, Default)]
struct Dialog {
    /// the window as it was drawn last
    rect: Option<Rect>,
    /// frames it has been the same size
    steady: u8,
    stage: Pivot,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
enum Pivot {
    /// the middle is where it was put
    #[default]
    Middle,
    /// this frame it is given its top left, from where it is
    Switch,
    /// it is placed by its top left, and moves as the user drags it
    TopLeft,
}

impl Dialog {
    /// what is known after a frame in which the window was `rect` and did or did not hold tabs.
    fn after(self, rect: Rect, tabbed: bool) -> Self {
        let steady = match self.rect {
            Some(r) if r.size() == rect.size() => self.steady.saturating_add(1),
            _ => 0,
        };
        let stage = match self.stage {
            Pivot::Middle if tabbed && steady >= 1 => Pivot::Switch,
            Pivot::Switch => Pivot::TopLeft,
            stage => stage,
        };
        Dialog {
            rect: Some(rect),
            steady,
            stage,
        }
    }
}

/// what a window gave back: what its title bar asked for, whether it was clicked and where it is.
pub struct Shown {
    pub actions: TitleActions,
    pub clicked: bool,
    pub rect: Rect,
    /// the title bar, as wide as the window inside its border.
    #[allow(dead_code)] // read by the tests
    pub bar: Rect,
    /// the height the contents asked for, with their margins.
    pub content: f32,
}

/// the height a window spends on its own border and title bar.
pub(crate) const CHROME: f32 = TITLE_BAR + 2.0 * FRAME;
/// the title bar's height, and the width of one of its buttons.
pub(crate) const TITLE_BAR: f32 = 20.0;
const TITLE_BUTTON: f32 = 16.0;
/// the border round a window, and the margin between it and the contents.
pub(crate) const FRAME: f32 = 3.0;
/// frames a window needs to take the size of its contents
const SETTLE: u8 = 3;

pub(crate) const MARGIN: f32 = 11.0;
/// where the title's text starts in the title bar: past the small icon and the gap after it.
const TITLE_TEXT: f32 = 23.0;

/// what a window adds to its contents: the frame and margins either side, and the title bar.
pub fn chrome() -> Vec2 {
    Vec2::new(2.0 * (FRAME + MARGIN), CHROME + 2.0 * MARGIN)
}

/// the least a resizable window can be: wide enough for its title bar, which holds the icon, the
/// whole title and three buttons, and as tall as its contents are with the parts that take extra
/// room shrunk to their least. `contents` is how tall they are then, with their margins.
pub fn least_size(ctx: &egui::Context, title: &str, contents: f32) -> Vec2 {
    let text = ctx.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(title.to_string(), theme::font(), WHITE)
            .size()
            .x
    });
    Vec2::new(
        FRAME + TITLE_TEXT + text + 3.0 * TITLE_BUTTON + 2.0 * FRAME,
        CHROME + contents,
    )
}

/// the least the contents can be without the room they are given, with their margins.
fn contents_least(before: Option<Room>) -> f32 {
    let least = match before {
        Some(r) if r.claimed > 0.0 => r.natural - r.claimed + r.claimed.min(controls::FILL_MIN),
        // tabs, whose frame is as big as the biggest page, are the least they are
        Some(r) if r.whole => r.natural,
        // nothing grows, so the contents scroll when the window is smaller: no smaller than
        // they are, or than a part that fills would be
        Some(r) => (r.natural - 2.0 * MARGIN).min(controls::FILL_MIN),
        // not measured yet: no floor, so a window that is small can open small
        None => 0.0,
    };
    least + 2.0 * MARGIN
}

/// the size a document window opens at, and how far it may shrink.
fn open_size(
    window: egui::Window<'_>,
    place: Placement,
    least: Vec2,
    before: Option<Room>,
    fit: bool,
    floor: f32,
) -> egui::Window<'_> {
    let max_height = place.area.height();
    match (place.growable, place.height) {
        (true, Some(h)) => window
            .default_size(Vec2::new(place.width, h.min(max_height)))
            .min_size(least),
        // no height to open at: it starts at nothing and takes the height of its contents
        (true, None) => {
            let window = window
                .default_size(Vec2::new(place.width, floor - 2.0 * FRAME))
                .min_size(least);
            match before {
                // until the user sizes it, it is as tall as its contents, or the floor, and
                // shrinks with them
                Some(r) if fit => window.max_size(Vec2::new(
                    f32::INFINITY,
                    (TITLE_BAR + r.natural)
                        .max(floor - 2.0 * FRAME)
                        .min(max_height - 2.0 * FRAME),
                )),
                _ => window,
            }
        }
        _ => window.default_width(place.width),
    }
}

/// the room above what the contents need on their own, for the growable part to take.
fn spare_room(before: Option<Room>, sized: bool, fit: bool, room: f32, floor: f32) -> f32 {
    match before {
        Some(r) if sized => (room - r.natural).max(0.0),
        // a window with less than it opens at gives the rest to its growable part
        Some(r) if fit => (floor - CHROME - r.natural).max(0.0),
        _ => 0.0,
    }
}

/// the user has sized the window once they press on its edge or corner.
fn grabbed_edge(ctx: &egui::Context, id: &str) -> bool {
    let Some(r) = ctx.memory(|m| m.area_rect(Id::unique(id))) else {
        return false;
    };
    // a press that has not moved sizes nothing
    let Some(p) = ctx.input(|i| {
        i.pointer
            .primary_down()
            .then(|| i.pointer.press_origin())
            .flatten()
            .filter(|p| {
                i.pointer
                    .latest_pos()
                    .is_some_and(|now| now.distance(*p) >= 1.0)
            })
    }) else {
        return false;
    };
    // not the title bar, whose buttons are not a resize
    p.y > r.top() + TITLE_BAR + FRAME
        && r.expand(FRAME * 2.0).contains(p)
        && !r.shrink(FRAME).contains(p)
        // not a window on top of this one
        && ctx
            .layer_id_at(p)
            .is_none_or(|layer| layer.id == Id::unique(id))
}

/// which of a window's edges a press at `p` takes hold of, by the zones egui grabs: a few pixels
/// either side of an edge, and a larger square around a corner.
fn grab_zones(ctx: &egui::Context, rect: Rect, p: Pos2) -> (bool, bool, bool, bool) {
    let interaction = ctx.global_style().interaction.clone();
    let (side, corner) = (
        interaction.resize_grab_radius_side,
        interaction.resize_grab_radius_corner,
    );
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

/// egui lays a window out once at the narrowest it goes when a resize begins, which it throws away
/// again: this is that pass, from the width the contents were given.
fn sizing_pass(ctx: &egui::Context, id: &str, width: f32, least: f32) -> bool {
    ctx.current_pass_index() == 0
        && ctx.input(|i| i.pointer.primary_down())
        && (width - (least - 2.0 * FRAME)).abs() < 0.01
        && egui::AreaState::load(ctx, layer(id).id)
            .and_then(|state| state.size)
            .is_some_and(|size| size.x > least + 1.0)
}

/// a drag on a window's edge or corner. a drag on the left or top edge moves the window's origin
/// as well as its size, and egui puts the window at the pointer whatever its least size is, so a
/// window already at its least slid sideways and its far edge moved. this holds the far edge.
#[derive(Clone, Copy)]
struct EdgeDrag {
    /// where the window was when the drag began
    start: Rect,
    left: bool,
    top: bool,
    /// the width and height the window has turned out to be when it was no smaller than it goes,
    /// nothing while it is as small as the pointer asks
    least: Vec2,
}

impl EdgeDrag {
    /// where the window goes this frame: the pointer's place, held so that the far edges stay
    /// where they were once the window cannot get any smaller.
    fn held(self, pos: Pos2) -> Pos2 {
        let least = self.least;
        Pos2::new(
            if self.left {
                pos.x.min(self.start.right() - least.x)
            } else {
                pos.x
            },
            if self.top {
                pos.y.min(self.start.bottom() - least.y)
            } else {
                pos.y
            },
        )
    }

    /// put the window where the drag holds it this frame: its place now, as egui has moved it on
    /// from the pointer. where that is, if the window is on screen.
    fn place<'a>(
        self,
        ctx: &egui::Context,
        id: &str,
        window: egui::Window<'a>,
    ) -> (egui::Window<'a>, Option<Pos2>) {
        match egui::AreaState::load(ctx, layer(id).id) {
            Some(state) => {
                let pos = state.left_top_pos();
                (window.current_pos(self.held(pos)), Some(pos))
            }
            None => (window, None),
        }
    }

    /// learn from the window drawn: when it is bigger than the pointer asked for it is as small as
    /// it goes, so it is held by its far edge at that size, which is more than its least when a
    /// control will not narrow. it is drawn again if that moved it. a window egui has kept on the
    /// desktop is not asking for anything.
    fn after(mut self, ctx: &egui::Context, key: Id, pos: Pos2, drawn: Rect, area: Rect) {
        let asked = self.start.max - pos;
        let held_in = |at: f32, edge: f32| at > edge + 0.5;
        let least = Vec2::new(
            if self.left && held_in(pos.x, area.left()) && drawn.width() > asked.x + 0.5 {
                drawn.width()
            } else {
                0.0
            },
            if self.top && held_in(pos.y, area.top()) && drawn.height() > asked.y + 0.5 {
                drawn.height()
            } else {
                0.0
            },
        );
        if least == self.least {
            return;
        }
        self.least = least;
        ctx.data_mut(|d| d.insert_temp(key, self));
        let moved = (drawn.max - self.start.max).abs();
        if (self.left && moved.x > 0.5) || (self.top && moved.y > 0.5) {
            ctx.request_discard("window held by its far edge");
        }
    }

    /// the drag, noted when it begins and followed until the button is let go.
    fn follow(ctx: &egui::Context, id: &str, key: Id) -> Option<Self> {
        if !ctx.input(|i| i.pointer.primary_down()) {
            ctx.data_mut(|d| d.remove::<Self>(key));
            return None;
        }
        if let Some(drag) = ctx.data(|d| d.get_temp::<Self>(key)) {
            return Some(drag);
        }
        // only a press that begins on the edge: egui grabs a few pixels either side of it
        let start = ctx.memory(|m| m.area_rect(Id::unique(id)))?;
        let p = ctx.input(|i| {
            i.pointer
                .primary_pressed()
                .then(|| i.pointer.press_origin())
        })??;
        let (left, right, top, bottom) = grab_zones(ctx, start, p);
        let on_it = (left || right || top || bottom)
            && ctx
                .layer_id_at(p)
                .is_none_or(|layer| layer.id == Id::unique(id));
        if !on_it {
            return None;
        }
        let drag = EdgeDrag {
            start,
            left,
            top,
            least: Vec2::ZERO,
        };
        ctx.data_mut(|d| d.insert_temp(key, drag));
        Some(drag)
    }
}

/// keep what a growable window's contents were, to share its room out the next frame, and draw
/// again until they settle.
fn remember_room(
    ctx: &egui::Context,
    id: &str,
    key: Id,
    before: Option<Room>,
    natural: f32,
    spare: controls::Spare,
    fit: bool,
) {
    let grabbed = grabbed_edge(ctx, id);
    let user = !fit || grabbed || before.is_some_and(|r| r.user);
    let now = Room {
        natural,
        claimed: spare.claimed,
        whole: spare.whole,
        user,
        // frames the contents have kept their height, for the window to catch up
        age: before
            .filter(|r| (r.natural - natural).abs() <= 0.5)
            .map_or(0, |r| r.age.saturating_add(1)),
    };
    ctx.data_mut(|d| d.insert_temp(key, now));
    if before.is_none_or(|r| r.age < SETTLE || (r.natural - natural).abs() > 0.5) {
        // the room to spare is worked out from this, so draw again with it
        ctx.request_repaint();
    }
}

/// draw the held-back status bar under the scrolling part, with its margins; how tall that took.
fn pinned_strip(ui: &mut Ui, pinned: Option<&view::Pinned>) -> f32 {
    let Some(pinned) = pinned else {
        return 0.0;
    };
    let t = view::pinned_amount(ui, pinned);
    let top = ui.cursor().top();
    egui::Frame::new()
        .inner_margin(Margin {
            left: MARGIN as i8,
            right: MARGIN as i8,
            top: 0,
            bottom: (MARGIN * t) as i8,
        })
        .show(ui, |ui| view::draw_pinned(ui, pinned));
    ui.cursor().top() - top
}

/// room to add under contents of `content_height` so that a window opens at its `floor`, none
/// when it is not a window that fits its contents.
fn pad_to_floor(fit: bool, floor: f32, content_height: f32, reserve: f32) -> f32 {
    if fit {
        (floor - CHROME - content_height - reserve).max(0.0)
    } else {
        0.0
    }
}

/// where a document window goes: kept on the desktop, at `start` from the first frame or whenever
/// it is put there afresh.
fn place_document<'a>(window: egui::Window<'a>, place: Placement) -> egui::Window<'a> {
    let window = window.constrain_to(place.area);
    if place.fresh {
        window.current_pos(place.start)
    } else {
        window.default_pos(place.start)
    }
}

/// where a dialog goes. `start` is where the middle of the window goes. a dialog of tabs is held
/// by its top left from its second frame at one size, so it stays where it is whatever its page.
fn place_dialog<'a>(window: egui::Window<'a>, place: Placement, seen: Dialog) -> egui::Window<'a> {
    match (seen.stage, seen.rect) {
        (Pivot::Switch, Some(r)) => window.pivot(Align2::LEFT_TOP).current_pos(r.min),
        (Pivot::TopLeft, _) => window.pivot(Align2::LEFT_TOP),
        _ if place.fresh => window.pivot(Align2::CENTER_CENTER).current_pos(place.start),
        _ => window.pivot(Align2::CENTER_CENTER).default_pos(place.start),
    }
}

/// what a dialog was last frame. a window that is not on screen yet, or whose place was forgotten,
/// opens in the middle again.
fn dialog_seen(ctx: &egui::Context, id: &str, key: Id) -> Dialog {
    if ctx.memory(|m| m.area_rect(Id::unique(id))).is_none() {
        return Dialog::default();
    }
    ctx.data(|d| d.get_temp(key)).unwrap_or_default()
}

/// keep where a dialog was drawn, and take the tab key press that drew it, for the next frame.
fn remember_dialog(ctx: &egui::Context, key: Id, seen: Option<Dialog>, drawn: Rect) {
    let tabbed = controls::take_tabbed(ctx);
    if let Some(seen) = seen {
        ctx.data_mut(|d| d.insert_temp(key, seen.after(drawn, tabbed)));
    }
}

/// the room the contents took. a fixed window keeps its width: the scroll bar takes room from the
/// contents, and a control that will not narrow must not widen the window.
fn taken_by(inner: &Ui, fixed: bool, width: f32) -> Rect {
    let mut taken = inner.min_rect();
    if fixed {
        taken.set_width(width);
    }
    taken
}

/// how many times egui may draw a window in one frame. when a resize begins egui draws it again,
/// and a window held by its far edge is drawn once more after that: allow for both while a window
/// is held, and only then.
fn allow_passes(ctx: &egui::Context, held: bool) {
    if held || !ctx.input(|i| i.pointer.primary_down()) {
        let passes = if held { 3 } else { 2 };
        ctx.options_mut(|o| {
            o.max_passes = std::num::NonZeroUsize::new(passes).unwrap_or(o.max_passes)
        });
    }
}

/// show a window. every window opens as tall as what is in it, with an 8 pixel margin all round,
/// and when that is more than the desktop it stops at the desktop and scrolls. a `dialog` has a
/// close button only, cannot be resized and opens centred on `place.start`; any other window has
/// the three buttons, can be resized and opens with its top left there, moved if need be to stay
/// on the desktop.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    place: Placement,
    active: bool,
    dialog: bool,
    content: impl FnOnce(&mut Ui),
) -> Option<Shown> {
    let mut actions = TitleActions::default();
    let mut content_height = 0.0;
    let width = place.width;
    let max_height = place.area.height();
    let fixed = place.fixed && !dialog;
    let growable = place.growable && !dialog && !fixed;
    // what the contents were last frame: how tall without the room they were given, and how tall
    // the parts that take room are on their own. none on the first frame, when a window is as
    // tall as its contents
    let key = Id::unique(("window-room", id));
    let before: Option<Room> = ctx.data(|d| d.get_temp(key));
    // how tall the status bar under the scrolling part was last frame
    let pin_key = Id::unique(("window-pin", id));
    let reserve: f32 = ctx.data(|d| d.get_temp(pin_key)).unwrap_or(0.0);
    // a growable window follows its contents, getting taller as they do, until the user sizes it
    let fit = growable && place.height.is_none() && !before.is_some_and(|r| r.user);
    let sized = growable && !fit;
    let floor = place.floor.min(max_height);
    let contents_least = contents_least(before);
    // never more than the desktop, whose room the contents then scroll in
    let mut least = least_size(ctx, title, contents_least);
    least.y = least.y.min(max_height);
    // only a window with a list, a log or a text can be resized: the rest are forms, as big as
    // their controls
    let mut window = egui::Window::new(title)
        .id(layer(id).id)
        .title_bar(false)
        .collapsible(false)
        .resizable(growable)
        .frame(
            egui::Frame::new()
                .fill(GRAY)
                .inner_margin(Margin::same(FRAME as i8)),
        );
    let dialog_key = Id::unique(("window-dialog", id));
    let seen = dialog_seen(ctx, id, dialog_key);
    window = if dialog {
        place_dialog(window, place, seen)
    } else {
        let window = place_document(window, place);
        match place.height.filter(|_| fixed) {
            Some(h) => window.fixed_size(Vec2::new(place.width, h.min(max_height))),
            None => open_size(window, place, least, before, fit, floor),
        }
    };
    let held_height = sized || (fixed && place.height.is_some());
    // a drag on the left or top edge keeps the far edges where they are
    let edge_key = Id::unique(("window-edge", id));
    let edge = EdgeDrag::follow(ctx, id, edge_key).filter(|_| growable);
    // egui draws a window again when a resize begins, and the window held by its far edge is drawn
    // once more after that: allow for both while a window is held, and only then
    allow_passes(ctx, edge.is_some());
    let mut wanted = None;
    if let Some(drag) = edge {
        (window, wanted) = drag.place(ctx, id, window);
    }
    let mut bar_rect = Rect::NOTHING;
    let mut origin = Pos2::ZERO;
    let shown = window.show(ctx, |ui| {
        origin = ui.cursor().min;
        // the title bar spans the window inside its 3 pixel border; the contents sit in
        // an 8 pixel margin on every side
        if dialog {
            ui.set_width(width - 2.0 * FRAME);
        }
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.y = 0.0;
        // the bar's room is taken now and it is drawn once the contents have settled the
        // window's width, so it always reaches the right edge
        let bar_top = ui.cursor().min;
        ui.allocate_space(Vec2::new(0.0, TITLE_BAR));
        // the contents are laid out in a space as tall as the desktop and the window takes the
        // size they use. asking the window's own ui would cap them at its last size, so a window
        // could never grow
        // a growable window gives its contents the room it has, so they follow its size
        let held = ui.available_height();
        let most = (max_height - CHROME).max(40.0);
        let room = if held_height {
            held.clamp(40.0, most)
        } else {
            most
        };
        // the room above what the contents need on their own, for the growable part to take
        let extra = spare_room(before, sized, fit, room, floor);
        // the status bar sits under the scrolling part, so that gets the room less its strip
        let scroll_room = (room - reserve).max(40.0);
        let from = ui.cursor().min;
        let room_width = ui.available_width();
        // when a resize begins egui lays the contents out once at the narrowest the window goes
        // and throws that pass away, to learn how far they can narrow. what the contents keep
        // from it (scroll bars, rooms) must not outlive it, so it is laid out under its own ids
        let sizing = growable && sizing_pass(ctx, id, room_width, least.x);
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    from,
                    Vec2::new(ui.available_width(), scroll_room),
                ))
                .layout(*ui.layout()),
        );
        let ((_, spare), pinned) = view::pin_status(ctx, || {
            controls::lend_room(ctx, extra, || {
                egui::ScrollArea::vertical()
                    .id_salt(("window-scroll", id, sizing))
                    .max_height(scroll_room)
                    .min_scrolled_height((least.y - CHROME - reserve).max(0.0))
                    .auto_shrink([false, !held_height])
                    .show(&mut inner, |ui| {
                        let framed = egui::Frame::new()
                            .inner_margin(Margin::same(MARGIN as i8))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing = spacing;
                                // what the contents keep under these ids is not shared with the narrow pass
                                ui.push_id(("sizing", sizing), content).inner
                            });
                        content_height = framed.response.rect.height();
                        // near empty contents leave the window the height it is meant to open at
                        let pad = pad_to_floor(fit, floor, content_height, reserve);
                        if pad > 0.0 {
                            ui.allocate_space(Vec2::new(0.0, pad));
                        }
                    });
            })
        });
        if growable && !sizing {
            let natural = content_height - (extra - spare.extra) + reserve;
            remember_room(ctx, id, key, before, natural, spare, fit);
        }
        // a fixed window keeps its width: the scroll bar takes room from the contents, and a
        // control that will not narrow must not widen the window
        ui.allocate_rect(taken_by(&inner, fixed, room_width), Sense::hover());
        let bar = Rect::from_min_max(
            bar_top,
            Pos2::new(ui.min_rect().right(), bar_top.y + TITLE_BAR),
        );
        bar_rect = bar;
        actions = title_bar(ui, bar, place.icon, title, active, dialog, fixed);
        // the status bar, if the contents end in one, sits below the scrolling part: on the
        // bottom edge of the window, and always in view
        let strip = pinned_strip(ui, pinned.as_ref());
        if !sizing {
            ctx.data_mut(|d| d.insert_temp(pin_key, strip));
        }
        content_height += strip;
    })?;
    // where the window was drawn. egui has moved its place on by now, to where a drag puts it
    // for the next frame, so its response is a frame ahead of the contents drawn in it
    let drawn = Rect::from_min_size(origin - Vec2::splat(FRAME), shown.response.rect.size());
    if let Some((drag, pos)) = edge.zip(wanted) {
        drag.after(ctx, edge_key, pos, drawn, place.area);
    }
    remember_dialog(ctx, dialog_key, dialog.then_some(seen), drawn);
    let painter = ctx.layer_painter(shown.response.layer_id);
    bevel(&painter, drawn, true);
    Some(Shown {
        actions,
        clicked: shown.response.is_pointer_button_down_on(),
        rect: drawn,
        bar: bar_rect,
        content: content_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;

    /// the height of a dialog holding `inner` points of content, on a screen `max` tall.
    fn shown(inner: f32, max: f32, dialog: bool) -> Rect {
        let ctx = egui::Context::default();
        let mut rect = None;
        for _ in 0..3 {
            testkit::run(&ctx, vec![], |ui| {
                let at = Pos2::new(480.0, 320.0);
                let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, max));
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: at,
                    width: 300.0,
                    area,
                    growable: false,
                    fixed: false,
                    floor: 0.0,
                    height: None,
                };
                rect = show(ui.ctx(), "t", "T", place, true, dialog, |ui| {
                    ui.allocate_space(Vec2::new(10.0, inner));
                })
                .map(|s| s.rect);
            });
        }
        rect.expect("shown")
    }

    /// the window and its title bar, for contents `wide` points across in a window opened at 300.
    fn bar_of(wide: f32, dialog: bool) -> (Rect, Rect) {
        let ctx = egui::Context::default();
        let mut out = None;
        for _ in 0..3 {
            testkit::run(&ctx, vec![], |ui| {
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: Pos2::new(480.0, 320.0),
                    width: 300.0,
                    area: Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 640.0)),
                    growable: false,
                    fixed: false,
                    floor: 0.0,
                    height: None,
                };
                out = show(ui.ctx(), "t", "T", place, true, dialog, |ui| {
                    ui.allocate_space(Vec2::new(wide, 20.0));
                })
                .map(|s| (s.rect, s.bar));
            });
        }
        out.expect("shown")
    }

    #[test]
    fn the_title_bar_reaches_the_right_edge_even_when_the_contents_are_wider_than_the_window() {
        for dialog in [true, false] {
            for wide in [10.0, 400.0] {
                let (rect, bar) = bar_of(wide, dialog);
                assert!(rect.width() > 300.0 - 1.0 || wide < 300.0);
                assert_eq!(
                    bar.right(),
                    rect.right() - FRAME,
                    "dialog {dialog} wide {wide}"
                );
                assert_eq!(
                    bar.left(),
                    rect.left() + FRAME,
                    "dialog {dialog} wide {wide}"
                );
            }
        }
    }

    #[test]
    fn a_dialog_keeps_the_same_margin_on_its_left_and_its_right() {
        let ctx = egui::Context::default();
        let mut out = None;
        for _ in 0..3 {
            testkit::run(&ctx, vec![], |ui| {
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: Pos2::new(480.0, 320.0),
                    width: 300.0,
                    area: Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 640.0)),
                    growable: false,
                    fixed: false,
                    floor: 0.0,
                    height: None,
                };
                let mut row = Rect::NOTHING;
                let rect = show(ui.ctx(), "t", "T", place, true, true, |ui| {
                    let w = ui.available_width();
                    row = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::hover()).0;
                })
                .map(|s| s.rect);
                out = rect.map(|r| (r, row));
            });
        }
        let (rect, row) = out.expect("shown");
        let left = row.left() - (rect.left() + FRAME);
        let right = (rect.right() - FRAME) - row.right();
        assert_eq!(left, MARGIN);
        assert_eq!(right, MARGIN);
    }

    fn height(inner: f32, max: f32) -> f32 {
        shown(inner, max, true).height()
    }

    #[test]
    fn a_fixed_window_is_the_size_it_is_given_whatever_its_contents() {
        // the last screen is shorter than the window: the outer size is held to the screen
        for (inner, screen, want) in [
            (10.0, 640.0, 250.0),
            (100.0, 640.0, 250.0),
            (900.0, 640.0, 250.0),
            (100.0, 200.0, 200.0),
        ] {
            let ctx = egui::Context::default();
            let mut rect = None;
            for _ in 0..4 {
                testkit::run(&ctx, vec![], |ui| {
                    let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, screen));
                    let place = Placement {
                        icon: "t",
                        fresh: false,
                        start: Pos2::new(100.0, 100.0),
                        width: 300.0,
                        area,
                        growable: false,
                        fixed: true,
                        floor: 0.0,
                        height: Some(250.0),
                    };
                    rect = show(ui.ctx(), "f", "F", place, true, false, |ui| {
                        ui.allocate_space(Vec2::new(10.0, inner));
                    })
                    .map(|s| s.rect);
                });
            }
            let size = rect.expect("shown").size();
            assert!(
                (size - Vec2::new(300.0, want)).abs().max_elem() <= 1.0,
                "{inner}: {size:?}"
            );
        }
    }

    /// the window and the top of the "ready" text of its status bar, after a few frames.
    fn with_status_bar(fixed: bool, inner: f32) -> (Rect, f32) {
        use lanlord_app::view::Node;
        let ctx = egui::Context::default();
        let nodes: Vec<Node<u8>> = vec![
            Node::Gap(1.0),
            Node::Gap(6.0),
            Node::Status {
                text: "ready".into(),
                error: false,
            },
        ];
        let (mut rect, mut text_top) = (Rect::NOTHING, 0.0);
        for _ in 0..40 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, testkit::SCREEN)),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: Pos2::new(100.0, 100.0),
                    width: 300.0,
                    area: Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 640.0)),
                    growable: !fixed,
                    fixed,
                    floor: 0.0,
                    height: Some(300.0),
                };
                rect = show(ui.ctx(), "s", "S", place, true, false, |ui| {
                    ui.allocate_space(Vec2::new(10.0, inner));
                    crate::view::show(ui, &nodes);
                })
                .map(|s| s.rect)
                .expect("shown");
            });
            out.textures_delta.clear();
            for clipped in &out.shapes {
                if let egui::epaint::Shape::Text(t) = &clipped.shape {
                    if t.galley.text() == "ready" {
                        text_top = t.pos.y;
                    }
                }
            }
        }
        (rect, text_top)
    }

    #[test]
    fn a_status_bar_sits_on_the_bottom_edge_of_a_fixed_window() {
        let (rect, top) = with_status_bar(true, 20.0);
        assert!(top > 0.0, "the bar was drawn");
        assert!(
            rect.bottom() - top < 30.0 + MARGIN - 8.0,
            "{top} in {rect:?}"
        );
    }

    #[test]
    fn a_status_bar_stays_in_view_below_contents_that_scroll() {
        for fixed in [true, false] {
            let (rect, top) = with_status_bar(fixed, 2000.0);
            assert!(top > 0.0, "the bar was drawn");
            assert!(
                rect.bottom() - top < 30.0 + MARGIN - 8.0,
                "{fixed}: {top} in {rect:?}"
            );
        }
    }

    #[test]
    fn a_fixed_window_keeps_its_width_when_a_control_will_not_narrow_beside_the_scroll_bar() {
        let ctx = egui::Context::default();
        let mut rect = None;
        for _ in 0..4 {
            testkit::run(&ctx, vec![], |ui| {
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: Pos2::new(100.0, 100.0),
                    width: 300.0,
                    area: Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 640.0)),
                    growable: false,
                    fixed: true,
                    floor: 0.0,
                    height: Some(250.0),
                };
                // tall enough for the scroll bar, and as wide as the window before the bar
                rect = show(ui.ctx(), "f", "F", place, true, false, |ui| {
                    ui.allocate_space(Vec2::new(300.0 - 2.0 * FRAME, 900.0));
                })
                .map(|s| s.rect);
            });
        }
        let width = rect.expect("shown").width();
        assert!((width - 300.0).abs() < 0.01, "{width}");
    }

    #[test]
    fn a_dialog_opens_centred_on_the_point_it_is_given() {
        // the screen is 960 by 640 and the dialog asks for its middle at (480, 320)
        for inner in [50.0, 150.0] {
            let rect = shown(inner, 600.0, true);
            assert!((rect.center().x - 480.0).abs() < 1.0, "{rect:?}");
            assert!((rect.center().y - 320.0).abs() < 1.0, "{rect:?}");
            assert!((rect.width() - 300.0).abs() < 1.0, "{rect:?}");
        }
    }

    #[test]
    fn a_dialog_is_as_tall_as_its_contents_and_no_taller_than_the_screen() {
        let short = height(50.0, 600.0);
        let tall = height(150.0, 600.0);
        assert!((tall - short - 100.0).abs() < 1.0, "{short} {tall}");
        // the margins above and below the contents, a 20 pixel title bar, a 3 pixel border
        assert!(
            (short - (50.0 + 2.0 * MARGIN + 20.0 + 6.0)).abs() < 1.5,
            "{short}"
        );
        // too much for a small screen: it stops at the screen and scrolls
        let clamped = height(2000.0, 300.0);
        assert!(clamped <= 300.0 + 1.0, "{clamped}");
        assert!(clamped > 250.0, "{clamped}");
    }

    #[test]
    fn a_document_window_grows_and_shrinks_with_its_contents_and_stays_on_the_desktop() {
        let ctx = egui::Context::default();
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 472.0));
        let run = |inner: f32| {
            let mut rect = None;
            for _ in 0..4 {
                testkit::run_on(&ctx, Vec2::new(800.0, 500.0), vec![], |ui| {
                    let place = Placement {
                        icon: "t",
                        fresh: false,
                        start: Pos2::new(110.0, 10.0),
                        width: 300.0,
                        area,
                        growable: false,
                        fixed: false,
                        floor: 0.0,
                        height: None,
                    };
                    rect = show(ui.ctx(), "doc", "Doc", place, true, false, |ui| {
                        ui.allocate_space(Vec2::new(10.0, inner));
                    })
                    .map(|s| s.rect);
                });
            }
            rect.unwrap()
        };
        let small = run(50.0);
        assert!(
            (small.height() - (50.0 + 2.0 * MARGIN + 26.0)).abs() < 1.5,
            "{small:?}"
        );
        let tall = run(300.0);
        assert!(
            (tall.height() - small.height() - 250.0).abs() < 1.5,
            "{tall:?}"
        );
        let huge = run(3000.0);
        assert!(huge.height() <= area.height() + 1.0, "{huge:?}");
        assert!(area.expand(1.0).contains_rect(huge), "{huge:?}");
        // and back down: it shrinks to the contents again
        let again = run(50.0);
        assert!(
            (again.height() - small.height()).abs() < 1.5,
            "{small:?} {again:?}"
        );
    }

    #[test]
    fn the_contents_sit_the_same_distance_from_the_frame_on_the_left_right_and_bottom() {
        // a 3 pixel border and an 8 pixel margin: 11 pixels, for a dialog and a document window
        for dialog in [true, false] {
            let ctx = egui::Context::default();
            let mut got = None;
            for _ in 0..4 {
                testkit::run_on(&ctx, Vec2::new(800.0, 500.0), vec![], |ui| {
                    let place = Placement {
                        icon: "t",
                        fresh: false,
                        start: Pos2::new(400.0, 200.0),
                        width: 300.0,
                        area: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 472.0)),
                        growable: false,
                        fixed: false,
                        floor: 0.0,
                        height: None,
                    };
                    let mut inner = Rect::NOTHING;
                    let shown = show(ui.ctx(), "t", "T", place, true, dialog, |ui| {
                        let wide = Vec2::new(ui.available_width(), 40.0);
                        inner = ui.allocate_exact_size(wide, Sense::hover()).0;
                    });
                    got = shown.map(|s| (s.rect, inner));
                });
            }
            let (frame, inner) = got.expect("shown");
            let left = inner.left() - frame.left();
            let right = frame.right() - inner.right();
            let bottom = frame.bottom() - inner.bottom();
            assert!(
                (left - FRAME - MARGIN).abs() < 0.5
                    && (right - FRAME - MARGIN).abs() < 0.5
                    && (bottom - FRAME - MARGIN).abs() < 0.5,
                "dialog {dialog}: left {left}, right {right}, bottom {bottom}"
            );
        }
    }

    #[test]
    fn a_growable_window_with_nothing_to_fill_is_as_tall_as_its_contents() {
        let ctx = egui::Context::default();
        let mut rect = None;
        for _ in 0..12 {
            testkit::run(&ctx, vec![], |ui| {
                let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 690.0));
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: Pos2::new(110.0, 12.0),
                    width: 380.0,
                    area,
                    growable: true,
                    fixed: false,
                    floor: 0.0,
                    height: None,
                };
                rect = show(ui.ctx(), "g", "G", place, true, false, |ui| {
                    ui.allocate_space(Vec2::new(10.0, 20.0));
                })
                .map(|s| s.rect);
            });
        }
        let rect = rect.unwrap();
        assert!(
            rect.height() < CHROME + 20.0 + 2.0 * MARGIN + 2.0,
            "{rect:?}"
        );
    }

    #[test]
    fn a_window_follows_contents_that_grow_a_few_frames_after_it_opens() {
        let ctx = egui::Context::default();
        let mut rect = None;
        // however late, and at whatever pace, the contents arrive (a slow machine loads them
        // later), the window stays as tall as they are
        for late in [4, 7, 12] {
            let id = format!("late{late}");
            for frame in 0..40 {
                let inner = if frame < late { 20.0 } else { 300.0 };
                testkit::run(&ctx, vec![], |ui| {
                    let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 690.0));
                    let place = Placement {
                        icon: "t",
                        fresh: false,
                        start: Pos2::new(110.0, 12.0),
                        width: 380.0,
                        area,
                        growable: true,
                        fixed: false,
                        floor: 0.0,
                        height: None,
                    };
                    rect = show(ui.ctx(), &id, "G", place, true, false, |ui| {
                        ui.allocate_space(Vec2::new(10.0, inner));
                    })
                    .map(|s| s.rect);
                });
            }
            let height = rect.unwrap().height();
            let wanted = CHROME + 300.0 + 2.0 * MARGIN;
            assert!((height - wanted).abs() < 2.0, "{late}: {height} {wanted}");
        }
    }

    #[test]
    fn a_window_follows_contents_that_shrink_and_a_click_elsewhere_does_not_size_it() {
        let ctx = egui::Context::default();
        let mut rect = None;
        for frame in 0..40 {
            let inner = if frame < 12 { 300.0 } else { 100.0 };
            // a press anywhere on the desktop in the first frames is not a resize
            let events = if frame == 1 {
                vec![egui::Event::PointerButton {
                    pos: Pos2::new(900.0, 600.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }]
            } else {
                vec![]
            };
            testkit::run(&ctx, events, |ui| {
                let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 690.0));
                let place = Placement {
                    icon: "t",
                    fresh: false,
                    start: Pos2::new(110.0, 12.0),
                    width: 380.0,
                    area,
                    growable: true,
                    fixed: false,
                    floor: 0.0,
                    height: None,
                };
                rect = show(ui.ctx(), "shrink", "S", place, true, false, |ui| {
                    ui.allocate_space(Vec2::new(10.0, inner));
                })
                .map(|s| s.rect);
            });
        }
        let height = rect.unwrap().height();
        let wanted = CHROME + 100.0 + 2.0 * MARGIN;
        assert!((height - wanted).abs() < 2.0, "{height} {wanted}");
    }

    #[test]
    fn a_tabbed_dialog_pivots_to_its_corner_once_its_size_has_settled() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(300.0, 200.0));
        let first = Dialog::default().after(rect, true);
        assert_eq!(
            first.stage,
            Pivot::Middle,
            "the first frame is still sizing"
        );
        let second = first.after(rect, true);
        assert_eq!(second.stage, Pivot::Switch);
        assert_eq!(second.after(rect, true).stage, Pivot::TopLeft);
        let after = second.after(rect, true).after(rect, false);
        assert_eq!(after.stage, Pivot::TopLeft, "it stays pinned to its corner");
    }

    #[test]
    fn a_dialog_without_tabs_keeps_to_the_middle() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(300.0, 200.0));
        let mut dialog = Dialog::default();
        for _ in 0..5 {
            dialog = dialog.after(rect, false);
        }
        assert_eq!(dialog.stage, Pivot::Middle);
        let grown = dialog.after(Rect::from_min_size(rect.min, Vec2::new(300.0, 260.0)), true);
        assert_eq!(grown.steady, 0, "a new size starts the count again");
    }
}
