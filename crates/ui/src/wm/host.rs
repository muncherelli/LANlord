//! the window host: an egui `Area` the wm drives, in place of `egui::Window`. the wm keeps each
//! window's position and drag, applies the pointer to a rect with `geometry` before the window is
//! laid out, and hands the contents a rect that only changes when the wm changes it. egui is left
//! with painting, the layer and its order.

use eframe::egui::{
    self, emath::GuiRounding as _, Align2, Area, Color32, Context, Frame, Id, Margin, Order, Pos2,
    Rect, Response, Role, Sense, Shape, Stroke, Ui, UiBuilder, UiKind, Vec2, WidgetInfo,
};

use super::geometry::{Drag, Section};
use crate::theme::GRAY;

/// the look of a window's frame. hard-coded to NT4 here, in one place, until the look supplies it.
pub(crate) struct Geometry {
    /// what the frame is filled with
    pub fill: Color32,
    /// the width of the border, which the contents also keep clear of
    pub border: f32,
}

pub(crate) fn frame() -> Geometry {
    Geometry {
        fill: GRAY,
        border: crate::windows::FRAME,
    }
}

/// the id of the caption's drag, to put on the title bar with `caption`.
fn caption_id(host: Id) -> Id {
    host.with("wm-caption")
}

/// the title bar as the part of a window that moves it. call it before the title bar's own
/// buttons are made, so that they are above it.
pub(crate) fn caption(ui: &Ui, bar: Rect, host: Id) {
    ui.interact(bar, caption_id(host), Sense::drag());
}

/// whether the pointer is down on the caption.
pub(crate) fn caption_down(ctx: &Context, host: Id) -> bool {
    ctx.read_response(caption_id(host))
        .is_some_and(|r| r.is_pointer_button_down_on())
}

/// where a window is put.
#[derive(Clone, Copy)]
enum Place {
    /// where to start, when the window has no place yet
    Default(Pos2),
    /// where it is, whatever it was
    Current(Pos2),
}

/// the eight places a window is held to resize it, the corners last so they win over the sides.
const HANDLES: [(&str, Section); 8] = [
    ("right", Section::Right),
    ("left", Section::Left),
    ("bottom", Section::Bottom),
    ("top", Section::Top),
    ("bottom-right", Section::BottomRight),
    ("top-right", Section::TopRight),
    ("bottom-left", Section::BottomLeft),
    ("top-left", Section::TopLeft),
];

fn handle_id(host: Id, name: &str) -> Id {
    host.with("wm-edge").with(name)
}

/// a press that has begun on an edge or the caption. an edge waits for the least width of the
/// contents, measured once, so that the drag is held to limits that stay the same all through it.
#[derive(Clone, Copy)]
struct Pending {
    section: Section,
    start: Rect,
    anchor: Pos2,
    pointer: Pos2,
}

#[derive(Clone, Copy)]
struct Held {
    drag: Drag,
    /// where the window's pivot was when the drag began
    anchor: Pos2,
    /// the size the contents turned out to need, when it was more than they said: the window
    /// goes no smaller than this for the rest of the drag
    learned: Vec2,
}

/// what the wm remembers of a window between frames.
#[derive(Clone, Copy, Default)]
struct State {
    /// the pivot's place, as it was last frame
    pos: Option<Pos2>,
    /// the window as it was drawn last frame
    rect: Option<Rect>,
    /// the size the window is asked to be, which the contents may make larger
    desired: Option<Vec2>,
    /// the size of the contents last frame
    last_content: Vec2,
    /// the least width the contents narrow to, measured when a drag begins
    min_width: Option<f32>,
    pending: Option<Pending>,
    drag: Option<Held>,
}

/// what a window gave back.
pub(crate) struct Hosted {
    pub response: Response,
    /// the window as it is drawn this frame, from the same rect as everything in it.
    pub rect: Rect,
    /// the pointer is down on the window or its caption.
    pub pressed: bool,
}

/// a window under the wm's hand.
#[must_use = "call `show`"]
pub(crate) struct Host {
    id: Id,
    title: String,
    place: Option<Place>,
    pivot: Align2,
    constrain: Option<Rect>,
    min_size: Vec2,
    max_size: Vec2,
    default_size: Vec2,
    area_size: Vec2,
    resizable: bool,
}

impl Host {
    pub fn new(id: Id, title: &str) -> Self {
        Host {
            id,
            title: title.to_string(),
            place: None,
            pivot: Align2::LEFT_TOP,
            constrain: None,
            min_size: Vec2::new(96.0, 32.0),
            max_size: Vec2::splat(f32::INFINITY),
            default_size: Vec2::new(340.0, 420.0),
            area_size: Vec2::NAN,
            resizable: true,
        }
    }

    pub fn default_pos(mut self, pos: Pos2) -> Self {
        self.place = Some(Place::Default(pos));
        self
    }

    pub fn current_pos(mut self, pos: Pos2) -> Self {
        self.place = Some(Place::Current(pos));
        self
    }

    pub fn pivot(mut self, pivot: Align2) -> Self {
        self.pivot = pivot;
        self
    }

    pub fn constrain_to(mut self, rect: Rect) -> Self {
        self.constrain = Some(rect);
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn default_size(mut self, size: Vec2) -> Self {
        self.default_size = size;
        self.area_size = size;
        self
    }

    pub fn default_width(mut self, width: f32) -> Self {
        self.default_size.x = width;
        self.area_size.x = width;
        self
    }

    pub fn min_size(mut self, size: Vec2) -> Self {
        self.min_size = size;
        self
    }

    pub fn max_size(mut self, size: Vec2) -> Self {
        self.max_size = size;
        self
    }

    /// this size and no resizing.
    pub fn fixed_size(mut self, size: Vec2) -> Self {
        self.default_size = size;
        self.min_size = size;
        self.max_size = size;
        self.resizable = false;
        self
    }

    pub fn show(self, ctx: &Context, contents: impl FnOnce(&mut Ui)) -> Option<Hosted> {
        let key = self.id.with("wm-state");
        let mut st: State = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
        let bounds = self.constrain.unwrap_or_else(|| ctx.content_rect());
        let max_size = self.max_size.min(bounds.size());
        // the wm's input, before anything is laid out
        let (down, pointer, origin) = ctx.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.latest_pos().or(i.pointer.interact_pos()),
                i.pointer.press_origin(),
            )
        });
        if !down {
            st.pending = None;
            st.drag = None;
            st.min_width = None;
        } else if st.pending.is_none() && st.drag.is_none() {
            let grabbed = self
                .resizable
                .then(|| {
                    HANDLES.iter().find(|(name, _)| {
                        ctx.read_response(handle_id(self.id, name))
                            .is_some_and(|r| r.dragged())
                    })
                })
                .flatten()
                .map(|h| h.1);
            let moved = ctx
                .read_response(caption_id(self.id))
                .is_some_and(|r| r.dragged())
                .then_some(Section::Caption);
            if let (Some(section), Some(start), Some(pointer)) =
                (grabbed.or(moved), st.rect, origin)
            {
                let anchor = st.pos.unwrap_or(start.min);
                st.pending = Some(Pending {
                    section,
                    start,
                    anchor,
                    pointer,
                });
            }
        }
        let mut measure = false;
        if let Some(p) = st.pending {
            if p.section == Section::Caption || st.min_width.is_some() {
                st.drag = Some(Held {
                    drag: Drag {
                        section: p.section,
                        start: p.start,
                        pointer_start: p.pointer,
                        min: Vec2::ZERO,
                        max: Vec2::splat(f32::INFINITY),
                    },
                    anchor: p.anchor,
                    learned: Vec2::ZERO,
                });
                st.pending = None;
            } else {
                measure = true;
            }
        }
        let (mut requested, mut forced) = (None, None);
        if let (Some(held), Some(at)) = (st.drag, pointer) {
            let mut drag = held.drag;
            // the limits are the window's own this frame, with what was learned of the contents
            if drag.section != Section::Caption {
                drag.max = max_size.max(drag.start.size());
                // a press never makes a window bigger than it is: its least counts only down to
                // the size it has
                let least = Vec2::new(
                    self.min_size.x.max(st.min_width.unwrap_or(0.0)),
                    self.min_size.y,
                );
                drag.min = least.min(drag.start.size()).max(held.learned);
            }
            let rect = drag.apply(at, bounds);
            forced = Some(held.anchor + (rect.min - drag.start.min));
            if drag.section != Section::Caption {
                requested = Some(rect.size());
            }
            ctx.move_to_top(egui::LayerId::new(Order::Middle, self.id));
        }
        if measure || st.drag.is_some() {
            // the narrow pass and a window held by its far edge are each drawn again at once:
            // leave room for them
            ctx.options_mut(|o| {
                o.max_passes =
                    std::num::NonZeroUsize::new(o.max_passes.get().max(3)).unwrap_or(o.max_passes)
            });
        }
        let pos = forced
            .or(match self.place {
                Some(Place::Current(p)) => Some(p),
                _ => None,
            })
            .or(st.pos)
            .or(match self.place {
                Some(Place::Default(p)) => Some(p),
                _ => None,
            });
        let mut area = Area::new(self.id)
            .kind(UiKind::Window)
            .accessible_name(self.title.as_str())
            .role(Role::Window)
            .pivot(self.pivot)
            .default_size(self.area_size)
            .sense(Sense::hover())
            .movable(false);
        if let Some(rect) = self.constrain {
            area = area.constrain_to(rect);
        }
        if let Some(pos) = pos {
            area = area.fixed_pos(pos);
        }
        let look = frame();
        let mut outer = Rect::NOTHING;
        let mut hover = [false; 8];
        let mut drag = [false; 8];
        let shown = area.show(ctx, |ui| {
            let framed = Frame::new().fill(look.fill).show(ui, |ui| {
                self.size_and_show(ui, &mut st, requested, measure, max_size, bounds, contents);
            });
            outer = framed.response.rect;
            if self.resizable {
                for (i, (name, section)) in HANDLES.iter().enumerate() {
                    let rect = zone(ui, outer, *section);
                    let response = ui.interact(rect, handle_id(self.id, name), Sense::drag());
                    response.widget_info(|| {
                        WidgetInfo::labeled(Role::Splitter, true, format!("Resize window {name}"))
                    });
                    hover[i] = response.hovered();
                    drag[i] = response.dragged();
                }
                paint_handles(ui, outer, hover, drag);
            }
        });
        let rect = Rect::from_min_size(outer.min, shown.response.rect.size());
        // a window bigger than it was asked to be is as small as it goes: it is held to that, and
        // drawn again by its far edge if it was moved
        if let (Some(held), Some(asked)) = (st.drag.as_mut(), requested) {
            let size = rect.size();
            let mut moved = false;
            for d in 0..2 {
                // only a side that moves with the pointer needs it: the far side just stays larger
                let near = [held.drag.section.left(), held.drag.section.top()][d];
                if near && size[d] > asked[d] + 0.5 && held.learned[d] < size[d] {
                    held.learned[d] = size[d];
                    moved = true;
                }
            }
            if moved {
                ctx.request_discard("wm held by its far edge");
            }
        }
        if let Some(state) = egui::AreaState::load(ctx, self.id) {
            st.pos = state.pivot_pos;
        }
        st.rect = Some(rect);
        ctx.data_mut(|d| d.insert_temp(key, st));
        Some(Hosted {
            pressed: shown.response.is_pointer_button_down_on() || caption_down(ctx, self.id),
            response: shown.response,
            rect,
        })
    }

    /// lay the contents out in the size the window is asked to be: the size the wm gave it,
    /// or the one it had, made larger by the contents if they need it.
    #[allow(clippy::too_many_arguments)]
    fn size_and_show(
        &self,
        ui: &mut Ui,
        st: &mut State,
        requested: Option<Vec2>,
        measure: bool,
        max_size: Vec2,
        bounds: Rect,
        contents: impl FnOnce(&mut Ui),
    ) {
        let look = frame();
        let position = ui.available_rect_before_wrap().min;
        let min = self.min_size;
        let max = max_size;
        let mut desired = st.desired.unwrap_or_else(|| {
            ui.request_repaint();
            self.default_size
                .max(min)
                .min(max)
                .min(ui.ctx().content_rect().size() - ui.spacing().window_margin.sum())
                .round_ui()
        });
        desired = desired.max(min).min(max).round_ui();
        let sized = requested.is_some();
        match requested {
            Some(mut size) => {
                if let Some(w) = st.min_width {
                    size.x = size.x.max(w);
                }
                desired = size;
            }
            None => desired = desired.max(st.last_content),
        }
        if !sized {
            desired = desired.max(min).min(max);
        }
        let inner = if measure {
            // the narrowest the contents go, drawn once and thrown away
            ui.ctx().request_discard("wm measured");
            Rect::from_min_size(position, Vec2::new(min.x, desired.y))
        } else {
            Rect::from_min_size(position, desired)
        };
        let mut clip = inner;
        clip.max = clip.max.max(inner.min + st.last_content);
        let clip = clip.intersect(ui.clip_rect());
        let mut builder = UiBuilder::new().max_rect(inner);
        if measure {
            builder = builder.sizing_pass();
        }
        let mut body = ui.new_child(builder);
        body.set_clip_rect(clip);
        Frame::NONE
            .inner_margin(Margin::same(look.border as i8))
            .show(&mut body, contents);
        if measure {
            st.min_width = Some(body.min_size().x);
        } else {
            st.last_content = body.min_size();
        }
        let mut size = st.last_content;
        for d in 0..2 {
            if self.resizable {
                desired[d] = desired[d].max(st.last_content[d]);
                size[d] = desired[d];
            }
        }
        if !measure {
            st.desired = Some(desired);
        }
        ui.advance_cursor_after_rect(Rect::from_min_size(body.min_rect().min, size));
        let _ = bounds;
    }
}

/// where a handle is, by the zones egui has always grabbed: a few pixels either side of an edge
/// and a larger square round a corner.
fn zone(ui: &Ui, outer: Rect, section: Section) -> Rect {
    let interaction = &ui.global_style().interaction;
    let (side, corner) = (
        interaction.resize_grab_radius_side,
        interaction.resize_grab_radius_corner,
    );
    let vertical = |a: Pos2, b: Pos2| Rect::from_min_max(a, b).expand2(Vec2::new(side, -corner));
    let horizontal = |a: Pos2, b: Pos2| Rect::from_min_max(a, b).expand2(Vec2::new(-corner, side));
    let square = |c: Pos2| Rect::from_center_size(c, Vec2::splat(2.0 * corner));
    match section {
        Section::Right => vertical(outer.right_top(), outer.right_bottom()),
        Section::Left => vertical(outer.left_top(), outer.left_bottom()),
        Section::Bottom => horizontal(outer.left_bottom(), outer.right_bottom()),
        Section::Top => horizontal(outer.left_top(), outer.right_top()),
        Section::BottomRight => square(outer.right_bottom()),
        Section::TopRight => square(outer.right_top()),
        Section::BottomLeft => square(outer.left_bottom()),
        _ => square(outer.left_top()),
    }
}

/// what hovering or dragging a handle shows: the cursor, the edge in the widget colour and the
/// grip in the bottom right corner, as egui draws them.
fn paint_handles(ui: &Ui, outer: Rect, hover: [bool; 8], drag: [bool; 8]) {
    let sections = HANDLES.map(|h| h.1);
    let of = |flags: [bool; 8]| {
        let any = |f: fn(Section) -> bool| sections.iter().zip(flags).any(|(s, on)| on && f(*s));
        [
            any(Section::left),
            any(Section::right),
            any(Section::top),
            any(Section::bottom),
        ]
    };
    let (hovered, dragged) = (of(hover), of(drag));
    let [left, right, top, bottom] = if dragged.contains(&true) {
        dragged
    } else {
        hovered
    };
    if left || right || top || bottom {
        let ctx = ui.ctx();
        if (left && top) || (right && bottom) {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        } else if (right && top) || (left && bottom) {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNeSw);
        } else if left || right {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        } else {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
    }
    let visuals = ui.visuals();
    let widget = if dragged.contains(&true) {
        visuals.widgets.active
    } else if hovered.contains(&true) {
        visuals.widgets.hovered
    } else {
        return;
    };
    let stroke = widget.bg_stroke;
    let half = stroke.width * 0.5;
    let rect = outer
        .shrink(half)
        .round_to_pixels(ui.pixels_per_point())
        .expand(half);
    let (min, max) = (rect.min, rect.max);
    let mut points = Vec::new();
    if right {
        points.extend([Pos2::new(max.x, min.y), Pos2::new(max.x, max.y)]);
    }
    if bottom {
        points.extend([Pos2::new(max.x, max.y), Pos2::new(min.x, max.y)]);
    }
    if left {
        points.extend([Pos2::new(min.x, max.y), Pos2::new(min.x, min.y)]);
    }
    if top {
        points.extend([Pos2::new(min.x, min.y), Pos2::new(max.x, min.y)]);
    }
    ui.painter().add(Shape::line(points, stroke));
    // the grip in the bottom right corner, lit by the same hover
    let corner_on = |flags: [bool; 8]| flags[4];
    let grip = if corner_on(drag) {
        visuals.widgets.active.fg_stroke.color
    } else if corner_on(hover) {
        visuals.widgets.hovered.fg_stroke.color
    } else {
        return;
    };
    let size = Vec2::splat(visuals.resize_corner_size);
    let area = Align2::RIGHT_BOTTOM
        .align_size_within_rect(size, outer)
        .translate(Vec2::splat(-2.0));
    let at = Align2::RIGHT_BOTTOM
        .pos_in_rect(&area)
        .round_to_pixels(ui.pixels_per_point());
    let line = Stroke::new(1.0, grip);
    let mut w = 2.0;
    while w <= area.width() && w <= area.height() {
        ui.painter()
            .line_segment([Pos2::new(at.x - w, at.y), Pos2::new(at.x, at.y - w)], line);
        w += 4.0;
    }
}
