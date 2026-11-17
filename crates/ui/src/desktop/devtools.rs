//! developer tools, compiled only with the `devtools` cargo feature: a small red bug in the
//! corner that opens a debug menu (a demo desktop on canned data, window-manager outlines, open
//! every window). release builds never contain any of it.

use std::rc::Rc;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Area, Color32, Frame, Id, Margin, Pos2, Rect, Sense, Stroke, Vec2,
};
use lanlord_app::desktop::Target;
use lanlord_app::gateway::Fake;
use lanlord_app::hostlist::HostList;
use lanlord_app::login::WebSession;
use lanlord_app::services::Services;

use super::Desktop;
use crate::admin_setup::AdminSetup;
use crate::login::{Auth, LoginBox, SignedIn};
use crate::theme::{self, GRAY, TEAL};

/// the height of the strip along the top that holds the bug, so the bug never covers a window,
/// an icon or the taskbar.
pub const STRIP: f32 = 22.0;

/// what the desktop was showing before the demo took over; leaving the demo puts it back.
pub struct Stash {
    services: Services,
    hosts: HostList,
    login: Option<LoginBox>,
    web: Option<Rc<dyn WebSession>>,
    auth: Option<Arc<dyn Auth>>,
    signed_in: Option<SignedIn>,
    admin_setup: Option<AdminSetup>,
    demo: bool,
}

#[derive(Default)]
pub struct Devtools {
    /// the debug menu is showing.
    pub menu: bool,
    /// outline every window's contents and padding, and say how big it measured and is.
    pub wm_debug: bool,
    /// the real desktop, kept while the demo desktop is showing.
    pub stash: Option<Stash>,
}

impl Desktop {
    /// is the demo desktop (canned data, no session) showing?
    pub fn in_demo(&self) -> bool {
        self.devtools.stash.is_some()
    }

    /// switch to a desktop on the made-up server: nothing here talks to the real one, and it is
    /// all labelled demo. the real logon, session and hosts come back when it is left.
    pub fn enter_demo(&mut self) {
        if self.in_demo() {
            return;
        }
        self.reset_session_ui();
        let gateway = Fake::new();
        *gateway.local.lock().unwrap() = true;
        let services = Services::new(Arc::new(gateway), crate::wake(&self.ctx));
        self.devtools.stash = Some(Stash {
            services: std::mem::replace(&mut self.services, services),
            hosts: std::mem::replace(&mut self.hosts, HostList::new(None)),
            login: self.login.take(),
            web: self.web.take(),
            auth: self.auth.take(),
            signed_in: self.signed_in.take(),
            admin_setup: self.admin_setup.take(),
            demo: std::mem::replace(&mut self.demo, true),
        });
    }

    /// leave the demo desktop and show what was there before. does nothing outside the demo.
    pub(crate) fn end_demo(&mut self) {
        let Some(stash) = self.devtools.stash.take() else {
            return;
        };
        self.services = stash.services;
        self.hosts = stash.hosts;
        self.login = stash.login;
        self.web = stash.web;
        self.auth = stash.auth;
        self.signed_in = stash.signed_in;
        self.admin_setup = stash.admin_setup;
        self.demo = stash.demo;
    }

    /// turn the window-manager outlines on or off.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn set_wm_debug(&mut self, on: bool) {
        self.devtools.wm_debug = on;
    }

    /// open every window there is, for looking at the window manager.
    pub fn open_all(&mut self) {
        let mut targets = self.icons.clone();
        targets.extend([
            Target::DateTime,
            Target::Components,
            Target::Welcome,
            Target::Changelog,
        ]);
        for target in targets {
            self.open(target);
        }
    }

    /// the strip along the top with the bug in its right corner; the debug menu hangs below it.
    pub(crate) fn devtools_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("devtools-strip")
            .exact_size(STRIP)
            .frame(Frame::new().fill(TEAL))
            .show(ui, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), STRIP), Sense::hover());
                let at = Rect::from_center_size(
                    Pos2::new(rect.right() - STRIP / 2.0 - 4.0, rect.center().y),
                    Vec2::splat(STRIP - 4.0),
                );
                let bug = ui
                    .interact(at, response.id.with("bug"), Sense::click())
                    .on_hover_text("Debug menu");
                paint_bug(ui.painter(), at.center());
                #[cfg(test)]
                theme::probe::record(ui, "bug", at, true);
                if bug.clicked() {
                    self.devtools.menu = !self.devtools.menu;
                }
            });
        if self.devtools.menu {
            self.debug_menu(&ctx);
        }
    }

    fn debug_menu(&mut self, ctx: &egui::Context) {
        let mut demo = self.in_demo();
        let was_demo = demo;
        let mut open_all = false;
        Area::new(Id::unique("devtools-menu"))
            .anchor(Align2::RIGHT_TOP, Vec2::new(-4.0, STRIP))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(GRAY)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        crate::controls::check(ui, &mut demo, "Demo desktop");
                        crate::controls::check(
                            ui,
                            &mut self.devtools.wm_debug,
                            "Window manager debug",
                        );
                        open_all = theme::button(ui, "Open all windows", false).clicked();
                    });
            });
        if demo && !was_demo {
            self.enter_demo();
        } else if !demo && was_demo {
            self.reset_session_ui();
        }
        if open_all {
            self.open_all();
        }
    }

    /// the window-manager overlay: each window's frame, its content box and its padding, and
    /// the size its contents measured against the size it actually is.
    pub(crate) fn wm_overlay(&self, ctx: &egui::Context) {
        if !self.devtools.wm_debug {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::unique("wm-debug"),
        ));
        for (target, rect, content) in &self.measured {
            painter.rect_stroke(
                *rect,
                0.0,
                Stroke::new(1.0, Color32::RED),
                egui::StrokeKind::Inside,
            );
            let body =
                Rect::from_min_max(rect.min + Vec2::new(3.0, 23.0), rect.max - Vec2::splat(3.0));
            painter.rect_stroke(
                body,
                0.0,
                Stroke::new(1.0, Color32::BLUE),
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                body.shrink(8.0),
                0.0,
                Stroke::new(1.0, Color32::GREEN),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.left_bottom() + Vec2::new(4.0, -4.0),
                Align2::LEFT_BOTTOM,
                format!(
                    "{}: measured {:.0}x{:.0}, actual {:.0}x{:.0}",
                    target.id(),
                    rect.width(),
                    content + crate::windows::CHROME,
                    rect.width(),
                    rect.height()
                ),
                theme::font(),
                Color32::RED,
            );
        }
    }
}

/// a small red bug: body, head, legs.
fn paint_bug(painter: &egui::Painter, c: Pos2) {
    let red = Color32::from_rgb(200, 0, 0);
    let dark = Color32::from_rgb(90, 0, 0);
    for dy in [-4.0, 0.0, 4.0] {
        for side in [-1.0, 1.0] {
            painter.line_segment(
                [
                    c + Vec2::new(side * 4.0, dy),
                    c + Vec2::new(side * 8.0, dy - 1.0),
                ],
                Stroke::new(1.0, dark),
            );
        }
    }
    painter.circle_filled(c + Vec2::new(0.0, 1.0), 5.0, red);
    painter.circle_filled(c + Vec2::new(0.0, -5.0), 3.0, dark);
    painter.line_segment(
        [c + Vec2::new(0.0, -3.0), c + Vec2::new(0.0, 6.0)],
        Stroke::new(1.0, dark),
    );
}

#[cfg(test)]
mod tests {
    use super::super::harness::*;
    use super::*;

    #[test]
    fn the_bug_opens_a_menu_and_open_all_windows_opens_them_all() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 2);
        assert!(!desktop.devtools.menu);
        assert!(press(&mut desktop, &ctx, "bug"), "the bug is in the corner");
        assert!(desktop.devtools.menu);
        assert!(press(&mut desktop, &ctx, "Open all windows"));
        frames(&mut desktop, &ctx, 3);
        for target in [
            Target::MyServer,
            Target::Welcome,
            Target::Changelog,
            Target::DateTime,
        ] {
            assert!(
                desktop.windows.iter().any(|w| w.target == target),
                "{target:?} is open"
            );
        }
    }

    #[test]
    fn the_bug_never_overlaps_a_window_or_the_taskbar() {
        for size in [Vec2::new(1280.0, 630.0), Vec2::new(800.0, 500.0)] {
            let (mut desktop, ctx, _) = desktop();
            desktop.open_all();
            frames_on(&mut desktop, &ctx, size, 4);
            let (bug, _) = theme::probe::find(&ctx, "bug").expect("the bug is drawn");
            assert!(
                bug.top() >= 0.0 && bug.bottom() <= STRIP,
                "{bug:?} sits in the strip"
            );
            assert!(!desktop.measured.is_empty());
            for (target, rect, _) in &desktop.measured {
                assert!(
                    !rect.intersects(bug),
                    "{target:?} {rect:?} covers the bug {bug:?}"
                );
                assert!(rect.top() >= STRIP, "{target:?} starts below the strip");
            }
            assert!(bug.bottom() < size.y - 30.0, "the taskbar is clear of it");
        }
    }

    #[test]
    fn the_window_manager_overlay_is_a_switch_that_draws_nothing_extra_to_the_layout() {
        let (mut desktop, ctx, _) = desktop();
        desktop.open_all();
        frames(&mut desktop, &ctx, 12);
        // a window whose contents are still arriving may keep growing; where and how wide each is
        // must not change
        let before: Vec<_> = desktop
            .measured
            .iter()
            .map(|m| (m.1.min, m.1.width()))
            .collect();
        desktop.set_wm_debug(true);
        frames(&mut desktop, &ctx, 3);
        let after: Vec<_> = desktop
            .measured
            .iter()
            .map(|m| (m.1.min, m.1.width()))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn the_demo_desktop_asks_the_real_server_nothing_and_is_left_on_logon() {
        let (mut desktop, ctx, real) = desktop();
        desktop.hosts.add("me@real").unwrap();
        frames(&mut desktop, &ctx, 2);
        desktop.enter_demo();
        assert!(desktop.in_demo());
        desktop.open_all();
        desktop.open_spec("all");
        frames(&mut desktop, &ctx, 12);
        std::thread::sleep(std::time::Duration::from_millis(50));
        frames(&mut desktop, &ctx, 3);
        assert_eq!(desktop.window_title, "LANlord (demo)");
        assert!(
            real.calls.lock().unwrap().is_empty(),
            "the real server was asked: {:?}",
            real.calls.lock().unwrap()
        );
        // a logon (or log off) ends the demo and brings the real desktop back, clean
        desktop.reset_session_ui();
        assert!(!desktop.in_demo());
        assert!(desktop.windows.is_empty());
        assert!(!desktop.demo);
        desktop.open(Target::MyServer);
        frames(&mut desktop, &ctx, 12);
        std::thread::sleep(std::time::Duration::from_millis(50));
        frames(&mut desktop, &ctx, 3);
        assert!(
            !real.calls.lock().unwrap().is_empty(),
            "after the demo the real server is the one asked"
        );
    }
}
