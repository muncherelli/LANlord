//! the LANlord desktop, drawn with egui so it looks the same in a native window on every
//! platform and in a browser.

mod admin_setup;
#[cfg(test)]
mod artwork;
mod clock_panel;
mod command_panel;
mod compat_alert;
mod components_window;
mod console;
mod controls;
mod desktop;
mod dialogs;
mod dns_panel;
mod form;
// generated art; the update animations in it are only drawn where lanlord updates itself
#[cfg_attr(not(feature = "native"), allow(dead_code))]
mod icon_data;
mod icons;
mod login;
mod logon_panel;
mod my_server;
mod nt4_panel;
mod nt4_window;
mod platform;
mod remote;
mod settings_editor;
mod task_manager;
#[cfg(test)]
mod testkit;
mod theme;
#[cfg(feature = "native")]
mod update_panel;
mod users_panel;
mod view;
mod view_wizard;
mod widgets;
mod windows;
mod wm;

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
            desktop.set_prefs(prefs::path());
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
/// (server mode: one server, logging on with the gateway's accounts) and talks to it through `/api/operations`; a build with the
/// `demo` feature shows the made-up demo server instead, and says so.
#[cfg(target_arch = "wasm32")]
pub async fn start_web(canvas: web_sys::HtmlCanvasElement) -> Result<(), wasm_bindgen::JsValue> {
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions {
                // no frame cap: the browser's own rate keeps window drags smooth, and idle
                // repaints are already stopped elsewhere
                dithering: false,
                ..eframe::WebOptions::default()
            },
            Box::new(|cc| {
                let mut desktop = desktop::Desktop::new(web_gateway(), &cc.egui_ctx, None);
                if cfg!(feature = "demo") {
                    desktop.mark_demo();
                } else {
                    // the gateway's own accounts log on in the page; a live session skips the box
                    desktop.require_web_login(std::rc::Rc::new(
                        lanlord_app::gateway::http::HttpSession,
                    ));
                }
                // `?open=gateway,nt4-domain@1` opens windows, like LANLORD_OPEN on a desktop,
                // but only the ones that are safe to open from a link
                #[cfg(feature = "devtools")]
                let open_all = apply_devtools_links(&mut desktop);
                #[cfg(not(feature = "devtools"))]
                let open_all = false;
                if let (Some(spec), false) = (query("open"), open_all) {
                    desktop.open_spec_after_logon(&safe_open_spec(&spec));
                }
                Ok(Box::new(desktop))
            }),
        )
        .await
}

/// `?demo=1` starts on the demo desktop, `?wmdebug=1` turns the window-manager outlines on and
/// `?menu=1` shows the debug menu, `?apply=1` plays an apply. `?open=all` opens every window; any other `?open=` list keeps
/// the safe filter a release has (no purge prompt from a link). true when `all` was handled.
/// only devtools builds look at these; a release keeps the safe list.
#[cfg(all(target_arch = "wasm32", feature = "devtools"))]
fn apply_devtools_links(desktop: &mut desktop::Desktop) -> bool {
    if query("demo").as_deref() == Some("1") {
        desktop.enter_demo();
    }
    desktop.set_wm_debug(query("wmdebug").as_deref() == Some("1"));
    desktop.set_menu(query("menu").as_deref() == Some("1"));
    // `?apply=1` plays a finished apply into the progress dialog, `?apply=2` with raw output
    if let Some(mode) = query("apply") {
        desktop.demo_apply(mode == "2");
    }
    let all = query("open").is_some_and(|spec| spec.trim() == "all");
    if all {
        desktop.open_spec_after_logon("all");
    }
    all
}

/// the made-up demo server in a demo build, the gateway the page came from otherwise.
#[cfg(target_arch = "wasm32")]
fn web_gateway() -> std::sync::Arc<dyn lanlord_app::gateway::Gateway> {
    #[cfg(feature = "demo")]
    {
        std::sync::Arc::new(lanlord_app::gateway::Fake::demo())
    }
    #[cfg(not(feature = "demo"))]
    std::sync::Arc::new(lanlord_app::gateway::http::Http::default())
}

/// the windows a link may open in the browser: no tab that asks before doing something (the
/// purge prompt is My Server's `@2`), and nothing else but these ids.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn safe_open_spec(spec: &str) -> String {
    const SAFE: [&str; 15] = [
        "menu",
        "taskmgr",
        "datetime",
        "my-server",
        "networks",
        "settings",
        "console",
        "hosts",
        "gateway",
        "docker",
        "nt4-domain",
        "web-proxy",
        "remote-commands",
        "dns",
        "dhcp",
    ];
    spec.split(',')
        .map(str::trim)
        .filter_map(|item| {
            let (id, tab) = item.split_once('@').unwrap_or((item, ""));
            if !SAFE.contains(&id) {
                return None;
            }
            let tab_ok = !tab.is_empty()
                && tab.len() == 1
                && tab != "2"
                && tab.chars().all(|c| c.is_ascii_digit());
            Some(if tab_ok {
                format!("{id}@{tab}")
            } else {
                id.to_string()
            })
        })
        .collect::<Vec<_>>()
        .join(",")
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
        .map(|(_, value)| {
            value
                .replace("%2C", ",")
                .replace("%2c", ",")
                .replace("%40", "@")
        })
}

#[cfg(test)]
mod tests {
    use super::safe_open_spec;

    /// a plain build (what a release is) has no developer tools; `tools/check-no-devtools.sh`
    /// checks the same for the release workflows.
    #[cfg(not(feature = "devtools"))]
    #[test]
    fn a_plain_build_has_no_developer_tools() {
        // a link may not open any window by id, `all` or the Welcome screen included
        assert_eq!(safe_open_spec("all,welcome,changelog"), "");
    }

    #[test]
    fn links_open_only_known_windows_and_never_the_purge_prompt() {
        assert_eq!(safe_open_spec("my-server@2,networks"), "my-server,networks");
        assert_eq!(
            safe_open_spec("my-server@1, nt4-domain@2"),
            "my-server@1,nt4-domain"
        );
        assert_eq!(safe_open_spec("developer,unknown,menu"), "menu");
        assert_eq!(safe_open_spec("dns@12,hosts@x"), "dns,hosts");
    }
}
