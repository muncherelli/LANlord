//! the LANlord desktop, drawn with egui so it looks the same in a native window on every
//! platform and in a browser.

// the browser build leaves out the desktop-only parts (sign-in, updates, rebuilds), so some of
// what they share goes unused there
#![cfg_attr(not(feature = "native"), allow(dead_code))]

mod admin_setup;
#[cfg(test)]
mod artwork;
mod clock_panel;
mod command_panel;
mod console;
mod controls;
mod desktop;
mod dialogs;
mod dns_panel;
mod form;
mod icon_data;
mod icons;
mod login;
mod logon_panel;
mod my_server;
mod networks_window;
mod nt4_panel;
mod nt4_window;
mod platform;
mod remote;
mod settings_editor;
#[cfg(test)]
mod testkit;
mod theme;
#[cfg(feature = "native")]
mod update_panel;
mod users_panel;
mod view;
mod widgets;
mod windows;

use eframe::egui;

/// a wake handle that asks egui for a new frame.
pub(crate) fn wake(ctx: &egui::Context) -> lanlord_app::wake::Wake {
    let ctx = ctx.clone();
    lanlord_app::wake::Wake::new(move || ctx.request_repaint())
}

/// open the window and block until it is closed.
#[cfg(feature = "native")]
pub fn run() -> anyhow::Result<()> {
    use std::sync::Arc;

    use anyhow::anyhow;
    use lanlord_core::{hosts, prefs, update};

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LANlord")
            .with_app_id("lanlord")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .map_err(|e| anyhow!("{e}"))?,
            )
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([800.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LANlord",
        options,
        Box::new(|cc| {
            let gateway = Arc::new(lanlord_app::gateway::Ssh);
            let mut desktop = desktop::Desktop::new(gateway, &cc.egui_ctx, hosts::config_path());
            // the logon box comes first; LANLORD_SCREEN=desktop skips it, to look at the windows
            if std::env::var("LANLORD_SCREEN").as_deref() != Ok("desktop") {
                desktop.require_login(Arc::new(login::SystemAuth::new()));
            }
            // a rebuilt dev build comes back to the server it was on and tries the agent again
            if let Ok(host) = std::env::var("LANLORD_LAST_HOST") {
                if std::env::var("LANLORD_RELAUNCHED").as_deref() == Ok("1") {
                    desktop.resume(&host);
                }
            }
            let prefs = prefs::path().map(|p| prefs::load(&p)).unwrap_or_default();
            desktop.watch_updates(
                update_panel::Updates::new(
                    Arc::new(update::System),
                    prefs::path(),
                    prefs.check_updates,
                ),
                update::dev_repo(),
                std::env::current_exe().ok(),
            );
            if std::env::var("LANLORD_FAKE_SERVER").as_deref() == Ok("1") {
                desktop.use_demo_server();
            }
            if let Ok(spec) = std::env::var("LANLORD_OPEN") {
                desktop.open_spec(&spec);
            }
            Ok(Box::new(desktop))
        }),
    )
    .map_err(|err| anyhow!("could not open the window: {err}"))
}

/// start the desktop on the page's canvas. the browser build is lanlord on the gateway itself
/// (server mode: one server, no logon box); until the web ui talks to the gateway it shows the
/// demo server.
#[cfg(target_arch = "wasm32")]
pub async fn start_web(canvas: web_sys::HtmlCanvasElement) -> Result<(), wasm_bindgen::JsValue> {
    use std::sync::Arc;

    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| {
                let gateway = lanlord_app::gateway::Fake::new();
                *gateway.local.lock().unwrap() = true;
                let mut desktop = desktop::Desktop::new(Arc::new(gateway), &cc.egui_ctx, None);
                // `?open=gateway,nt4-domain@1` opens windows, like LANLORD_OPEN on a desktop
                if let Some(spec) = query("open") {
                    desktop.open_spec(&spec);
                }
                Ok(Box::new(desktop))
            }),
        )
        .await
}

/// the value of `name` in the page address's query string.
#[cfg(target_arch = "wasm32")]
fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.replace("%2C", ",").replace("%40", "@"))
}
