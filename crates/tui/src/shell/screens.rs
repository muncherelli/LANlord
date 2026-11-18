//! every screen, window and dialog of the demo desktop, drawn at fixed sizes, for the layout
//! tests: snapshots of what is on screen, and checks that the windows are whole and nothing is
//! cut off.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lanlord_app::gateway::Fake;
use lanlord_app::login::fake::FakeAuth;
use lanlord_app::menu::{Action, Row};
use lanlord_app::networks_window::{self, NetworksWindow};
use lanlord_core::update::{Source, Update};

use super::{
    AdminSetup, Arc, Buffer, Form, HostList, Modal, Palette, Power, Rect, Shell, Target, MODULES,
};
use crate::paint::{cut, lines};
use crate::view::{Part, Region};

/// the smallest terminal the tui supports, and a large one.
pub const SMALL: (u16, u16) = (80, 24);
pub const LARGE: (u16, u16) = (160, 45);
/// the sizes the structure checks run at.
const SIZES: [(u16, u16); 3] = [SMALL, (100, 30), LARGE];

/// a demo desktop with a fixed clock, its first frame drawn at `size`.
fn desktop(size: (u16, u16)) -> Shell {
    desktop_with(size, Fake::new())
}

/// a demo desktop over `fake`. lease times are written in UTC, so a snapshot reads the same
/// in every time zone.
fn desktop_with(size: (u16, u16), fake: Fake) -> Shell {
    let mut shell = Shell::new(Arc::new(fake), HostList::new(None), Palette::default());
    shell.clock = Some("12:00 PM".into());
    shell.use_demo_server();
    shell.networks = NetworksWindow::default().with_time(networks_window::utc_time);
    draw(&mut shell, size);
    shell
}

/// draw frames at `size` until the picture stops changing, so background reads have landed.
fn draw(shell: &mut Shell, size: (u16, u16)) -> Buffer {
    let area = Rect::new(0, 0, size.0, size.1);
    let started = Instant::now();
    let (mut last, mut steady) = (String::new(), 0);
    loop {
        shell.poll();
        let mut buf = Buffer::empty(area);
        shell.render(area, &mut buf);
        let text = text_of(&buf);
        steady = if text == last { steady + 1 } else { 0 };
        if steady >= 4 || started.elapsed() > Duration::from_secs(5) {
            return buf;
        }
        last = text;
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// the buffer as text, one line per row, without trailing blanks.
fn text_of(buf: &Buffer) -> String {
    lines(buf)
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// a desktop with `target` open.
fn window(size: (u16, u16), target: Target) -> Shell {
    let mut shell = desktop(size);
    shell.open(target);
    shell
}

fn module(id: &str) -> Target {
    Target::Module(MODULES.iter().position(|m| m.id == id).unwrap())
}

/// one named screen, built fresh for a terminal size.
struct Scene {
    name: String,
    build: Box<dyn Fn((u16, u16)) -> Shell>,
}

fn scene(name: impl Into<String>, build: impl Fn((u16, u16)) -> Shell + 'static) -> Scene {
    // the red bug of a development build is left out, so these pictures are the same with and
    // without the developer tools
    scene_with_bug(name, move |size| {
        let mut shell = build(size);
        shell.hide_bug();
        shell
    })
}

/// like `scene`, with the red bug showing in a development build.
fn scene_with_bug(name: impl Into<String>, build: impl Fn((u16, u16)) -> Shell + 'static) -> Scene {
    Scene {
        name: name.into(),
        build: Box::new(build),
    }
}

/// a checkout and a build to rebuild from; neither exists, which is what the failing rebuild
/// scene needs.
pub(super) fn develop(shell: &mut Shell) {
    shell.rebuild.watch(
        Some(PathBuf::from("/srv/lanlord")),
        Some(PathBuf::from("/srv/lanlord/target/debug/lanlord")),
    );
}

/// what a check for updates found.
pub(super) fn news() -> Update {
    Update {
        source: Source::Git,
        headline: "LANlord 0.2.0".into(),
        lines: vec!["Adds the debug menu to the terminal.".into()],
    }
}

/// the row of the LANlord menu that opens the Developer submenu.
pub(super) fn developer_row(shell: &Shell) -> usize {
    shell
        .menu_state()
        .iter()
        .position(|row| matches!(row, Row::Item(item) if item.action == Action::Developer))
        .expect("a development build has a Developer row")
}

/// the logon box, the first-administrator screen, the desktop, its menu and boxes, then every
/// window.
fn scenes() -> Vec<Scene> {
    let mut all = vec![
        scene("logon", |_| {
            let mut shell = Shell::new(
                Arc::new(Fake::new()),
                HostList::new(None),
                Palette::default(),
            );
            shell.require_login(Arc::new(FakeAuth::default()));
            shell
        }),
        scene("logon-this-server", |_| {
            let auth = FakeAuth::default();
            *auth.here.lock().unwrap() = Some("ann@this-server.localhost".into());
            let mut shell = Shell::new(
                Arc::new(Fake::new()),
                HostList::new(None),
                Palette::default(),
            );
            shell.require_login(Arc::new(auth));
            shell
        }),
        scene("logon-another-server", |_| {
            let auth = FakeAuth::default();
            *auth.here.lock().unwrap() = Some("ann@this-server.localhost".into());
            let mut shell = Shell::new(
                Arc::new(Fake::new()),
                HostList::new(None),
                Palette::default(),
            );
            shell.require_login(Arc::new(auth));
            if let Some(login) = shell.login.as_mut() {
                login.update(lanlord_app::login::Msg::Where(1), &[]);
            }
            shell
        }),
        scene("first-administrator", |size| {
            let mut shell = desktop(size);
            shell.admin_setup = Some(AdminSetup::new());
            shell
        }),
        scene("desktop", desktop),
        scene("menu", |size| {
            let mut shell = desktop(size);
            shell.menu = Some(0);
            shell
        }),
        scene("about", |size| {
            let mut shell = desktop(size);
            let text = "LANlord\nRetro networks, run from one desktop.";
            shell.modal = Some(Modal::About(Form::new("about", "About LANlord", text)));
            shell
        }),
        scene("confirm", |size| {
            let mut shell = desktop(size);
            let form = Form::new("power", "Reboot server", "Restart demo@gateway now?");
            shell.modal = Some(Modal::Confirm(Power::Reboot, "demo@gateway".into(), form));
            shell
        }),
        scene("add-host", |size| {
            let mut shell = desktop(size);
            let form =
                Form::new("add-host", "Add a host", "Gateway as user@host:").text("Host", "");
            shell.modal = Some(Modal::AddHost(form));
            shell
        }),
    ];
    all.extend(maintenance_scenes());
    for target in [
        Target::MyServer,
        Target::Networks,
        Target::Console,
        Target::Hosts,
        Target::Settings,
        Target::DateTime,
        Target::Components,
        Target::Welcome,
        Target::Changelog,
        Target::MyAccount,
        Target::Permissions,
        Target::Progress,
        Target::TaskManager,
    ] {
        all.push(scene(target.id(), move |size| window(size, target)));
    }
    for (index, module) in MODULES.iter().enumerate() {
        all.push(scene(format!("module-{}", module.id), move |size| {
            window(size, Target::Module(index))
        }));
    }
    all.extend(window_boxes());
    for tab in 1..3 {
        all.push(scene(format!("nt4-domain-tab{tab}"), move |size| {
            let mut shell = window(size, module("nt4-domain"));
            shell.nt4_tab = tab;
            shell
        }));
    }
    all
}

/// the update notice, the Developer submenu, rebuilding and the developer tools.
fn maintenance_scenes() -> Vec<Scene> {
    #[allow(unused_mut)]
    let mut all = vec![
        scene("developer-menu", |size| {
            let mut shell = desktop(size);
            develop(&mut shell);
            shell.menu = Some(developer_row(&shell));
            shell.submenu = Some(0);
            shell
        }),
        scene("rebuild", |size| {
            let mut shell = desktop(size);
            develop(&mut shell);
            shell.do_action(Action::Rebuild);
            shell
        }),
        scene("rebuild-failed", |size| {
            let mut shell = desktop(size);
            shell.show_failure("the checkout is gone");
            shell
        }),
        scene("update-available", |size| {
            let mut shell = desktop(size);
            shell.updates.arrived(Some(news()));
            shell
        }),
        scene("update-box", |size| {
            let mut shell = desktop(size);
            shell.updates.arrived(Some(news()));
            shell.updates.open_dialog();
            shell
        }),
    ];
    #[cfg(feature = "devtools")]
    all.extend(devtools_scenes());
    all
}

/// the red bug, its menu, the outlines and the demo desktop.
#[cfg(feature = "devtools")]
fn devtools_scenes() -> Vec<Scene> {
    vec![
        scene_with_bug("debug-button", desktop),
        scene_with_bug("debug-menu", |size| {
            let mut shell = desktop(size);
            shell.devtools.menu = Some(0);
            shell
        }),
        scene_with_bug("debug-window-outlines", |size| {
            let mut shell = window(size, Target::MyServer);
            shell.pick_debug(1);
            shell
        }),
        scene_with_bug("debug-logon", |_| {
            let mut shell = Shell::new(
                Arc::new(Fake::new()),
                HostList::new(None),
                Palette::default(),
            );
            shell.require_login(Arc::new(FakeAuth::default()));
            shell
        }),
        scene_with_bug("debug-demo-desktop", |size| {
            let mut shell = desktop(size);
            shell.pick_debug(0);
            shell.devtools.menu = Some(0);
            shell
        }),
    ]
}

/// the boxes that open from the Networks, My Account and Permissions windows.
fn window_boxes() -> Vec<Scene> {
    use lanlord_app::account_window::Msg as AccountMsg;
    use lanlord_app::networks_window::Msg as Networks;
    use lanlord_app::permissions_window::Msg as PermissionsMsg;
    const HOST: &str = "demo@gateway";
    vec![
        scene("networks-lanlord", |size| {
            let fake = Fake::new();
            *fake.stored.lock().unwrap() = super::window_tests::NETWORKS.into();
            let mut shell = desktop_with(size, fake);
            shell.open(Target::Networks);
            draw(&mut shell, size);
            shell
                .networks
                .update(Networks::Tab(1), &shell.services, HOST, 0.0);
            shell
        }),
        scene("networks-properties", |size| {
            let mut shell = window(size, Target::Networks);
            draw(&mut shell, size);
            shell
                .networks
                .update(Networks::Properties, &shell.services, HOST, 0.0);
            shell
        }),
        scene("networks-connection", |size| {
            let mut shell = window(size, Target::Networks);
            draw(&mut shell, size);
            shell
                .networks
                .update(Networks::Details, &shell.services, HOST, 0.0);
            shell
        }),
        scene("change-password", |size| {
            let mut shell = window(size, Target::MyAccount);
            draw(&mut shell, size);
            shell
                .account
                .update(AccountMsg::ChangePassword, &shell.services, HOST);
            shell
        }),
        scene("add-user", |size| {
            let mut shell = window(size, Target::Permissions);
            draw(&mut shell, size);
            shell
                .permissions
                .update(PermissionsMsg::AddUser, &shell.services, HOST, true);
            shell
        }),
    ]
}

/// the desktop with a handful of windows open at once, the last one on top.
fn crowded(size: (u16, u16)) -> Shell {
    let mut shell = desktop(size);
    for target in [
        Target::Hosts,
        Target::MyServer,
        Target::Settings,
        module("dns"),
        Target::Console,
    ] {
        shell.open(target);
        draw(&mut shell, size);
    }
    shell
}

/// the picture of a scene. the Date/Time window counts the server's seconds, so those are
/// masked: a snapshot must not depend on when it was taken.
fn picture(scene: &Scene, size: (u16, u16)) -> String {
    let mut shell = (scene.build)(size);
    let mut text = text_of(&draw(&mut shell, size));
    for digit in '0'..='9' {
        text = text
            .replace(&format!("16:12:0{digit}"), "16:12:0X")
            .replace(&format!("0{digit} ▲ ▼   PM"), "0X ▲ ▼   PM");
    }
    text
}

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("shell")
        .join("snapshots")
}

/// every scene at the small and the large size looks like its stored snapshot. run with
/// `LANLORD_UPDATE_SNAPSHOTS=1` to write the snapshots again after a change to the layout, and
/// read the diff.
#[test]
fn every_screen_looks_like_its_snapshot() {
    let update = std::env::var("LANLORD_UPDATE_SNAPSHOTS").is_ok();
    let mut wrong = Vec::new();
    for scene in scenes() {
        for size in [SMALL, LARGE] {
            let shown = picture(&scene, size).replace("\r\n", "\n") + "\n";
            let file = snapshot_dir().join(format!("{}-{}x{}.txt", scene.name, size.0, size.1));
            if update {
                std::fs::create_dir_all(snapshot_dir()).unwrap();
                std::fs::write(&file, &shown).unwrap();
                continue;
            }
            match std::fs::read_to_string(&file) {
                Ok(stored) if stored.replace("\r\n", "\n") == shown => {}
                Ok(stored) => {
                    let line = stored
                        .lines()
                        .zip(shown.lines())
                        .position(|(a, b)| a != b)
                        .map_or(0, |n| n + 1);
                    wrong.push(format!(
                        "{} differs at line {line}:\n{shown}",
                        file.display()
                    ));
                }
                Err(_) => wrong.push(format!("{} is missing:\n{shown}", file.display())),
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// is the window frame at `rect` whole, apart from cells a window above it covers?
fn broken_frame(buf: &Buffer, rect: Rect, above: &[Rect]) -> Vec<String> {
    let mut broken = Vec::new();
    let mut expect = |x: u16, y: u16, glyph: &str| {
        if above.iter().any(|r| r.contains((x, y).into())) {
            return;
        }
        let found = buf[(x, y)].symbol();
        if found != glyph {
            broken.push(format!("({x}, {y}) is {found:?}, not {glyph:?}"));
        }
    };
    let (left, right, top, bottom) = (rect.left(), rect.right() - 1, rect.top(), rect.bottom() - 1);
    for x in left + 1..right {
        expect(x, bottom, "─");
    }
    for y in top + 1..bottom {
        expect(left, y, "│");
        expect(right, y, "│");
    }
    // the top line carries the title and the controls, so only its corners are checked
    expect(left, top, "┌");
    expect(right, top, "┐");
    expect(left, bottom, "└");
    expect(right, bottom, "┘");
    broken
}

/// every window lies on the desktop and has all four edges drawn: none was painted over by a
/// window under it, or cut by the screen, and a box over the desktop is whole too.
#[test]
fn windows_stay_on_the_desktop_with_their_borders_whole() {
    let mut scenes = vec![scene("crowded", crowded)];
    scenes.extend(scenes_for_frames());
    for size in SIZES {
        let desktop = Rect::new(0, 0, size.0, size.1 - 1);
        for Scene { name, build } in &scenes {
            let mut shell = build(size);
            let buf = draw(&mut shell, size);
            let mut frames: Vec<Rect> = shell
                .hits
                .windows
                .iter()
                .map(|(_, rect, _, _)| *rect)
                .collect();
            if let Some((rect, _)) = &shell.hits.dialog {
                frames.push(*rect);
            }
            // windows that overlap hide each other's edges, so only a lone scene is checked
            if frames.len() == 1 {
                let broken = broken_boxes(&buf, &[]);
                assert!(
                    broken.is_empty(),
                    "{name} at {size:?}: a box is broken: {broken:?}\n{}",
                    text_of(&buf)
                );
            }
            for (n, rect) in frames.iter().enumerate() {
                assert!(
                    desktop.contains(rect.as_position())
                        && rect.right() <= desktop.right()
                        && rect.bottom() <= desktop.bottom(),
                    "{name} at {size:?}: {rect:?} is not inside the desktop {desktop:?}"
                );
                let broken = broken_frame(&buf, *rect, &frames[n + 1..]);
                assert!(
                    broken.is_empty(),
                    "{name} at {size:?}: frame {rect:?} is broken: {broken:?}\n{}",
                    text_of(&buf)
                );
            }
        }
    }
}

/// the scenes that have a frame to check: everything but the logon box, which has no desktop.
fn scenes_for_frames() -> Vec<Scene> {
    scenes()
}

/// the bottom edges of boxes inside the windows (groups, tabs and pages) that do not run from
/// a `└` straight to a `┘`, and the corners with nothing above them.
fn broken_boxes(buf: &Buffer, covered: &[Rect]) -> Vec<String> {
    let mut broken = Vec::new();
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if buf[(x, y)].symbol() != "└" || covered.iter().any(|r| r.contains((x, y).into())) {
                continue;
            }
            let mut end = x + 1;
            while end < area.right() && buf[(end, y)].symbol() == "─" {
                end += 1;
            }
            if end >= area.right() || buf[(end, y)].symbol() != "┘" {
                broken.push(format!(
                    "the box edge at ({x}, {y}) does not end in a corner"
                ));
            }
            if y == area.top() || !matches!(buf[(x, y - 1)].symbol(), "│" | "┌" | "├") {
                broken.push(format!("the corner at ({x}, {y}) has no side above it"));
            }
        }
    }
    broken
}

/// at 80 by 24 no text on any screen is cut short: a label, a value or a button that does not
/// fit its room wraps or makes the window bigger instead.
#[test]
fn no_text_is_cut_off_at_80_by_24() {
    cut::take();
    let mut all = scenes();
    all.push(scene("crowded", crowded));
    let mut cut_off = Vec::new();
    for scene in &all {
        let mut shell = (scene.build)(SMALL);
        cut::take();
        draw(&mut shell, SMALL);
        // the console and the first frames may draw while loading; only the last frame counts
        cut::take();
        let area = Rect::new(0, 0, SMALL.0, SMALL.1);
        shell.render(area, &mut Buffer::empty(area));
        for text in cut::take() {
            cut_off.push(format!("{}: {text:?}", scene.name));
        }
    }
    assert!(cut_off.is_empty(), "{}", cut_off.join("\n"));
}

/// how many blank cells lie next to `(x, y)` on one side before something is drawn.
fn blanks(buf: &Buffer, x: u16, y: u16, step: i32) -> u16 {
    let mut count = 0;
    let mut at = i32::from(x) + step;
    while at >= 0 && at < i32::from(buf.area.width) && buf[(at as u16, y)].symbol() == " " {
        count += 1;
        at += step;
    }
    count
}

/// the regions of every window and of the dialog, one list each. a dialog has the whole
/// desktop's input, and may cover part of a window, so the windows behind it do not count.
fn region_lists(shell: &Shell) -> Vec<Vec<Region>> {
    if let Some((_, regions)) = &shell.hits.dialog {
        return vec![regions.clone()];
    }
    let lists: Vec<Vec<Region>> = shell
        .hits
        .windows
        .iter()
        .map(|(_, _, _, regions)| regions.clone())
        .collect();
    lists
}

/// the buttons of each button row among `regions`, left to right: rows of two or more only.
fn button_rows(regions: Vec<Region>) -> Vec<Vec<Rect>> {
    let mut rows: Vec<(Vec<usize>, Vec<Rect>)> = Vec::new();
    for region in regions
        .into_iter()
        .filter(|region| matches!(region.part, Part::Press(_)))
    {
        match rows.iter_mut().find(|(path, _)| *path == region.path) {
            Some((_, rects)) => rects.push(region.rect),
            None => rows.push((region.path.clone(), vec![region.rect])),
        }
    }
    rows.into_iter()
        .map(|(_, mut rects)| {
            rects.sort_by_key(|rect| rect.x);
            rects
        })
        .filter(|rects| rects.len() > 1)
        .collect()
}

/// what is wrong with one button row, if anything: its buttons are one width on one line, a
/// whole number of buttons apart, and it sits flush to the right or the left of its box.
fn row_faults(buf: &Buffer, rects: &[Rect]) -> Vec<&'static str> {
    let (first, last) = (rects[0], rects[rects.len() - 1]);
    let mut faults = Vec::new();
    if rects.iter().any(|r| r.y != first.y) {
        faults.push("are not on one line");
    }
    // a disabled button has no region, so a gap may span one
    if rects.windows(2).any(|w| w[1].x <= w[0].right()) {
        faults.push("touch or overlap");
    }
    let right = blanks(buf, last.right() - 1, last.y, 1) <= 1;
    let left = blanks(buf, first.x, first.y, -1) <= 1;
    if !right && !left {
        faults.push("are not flush to a side");
    }
    faults
}

/// every row of two or more buttons, in every window and dialog, has buttons of one width a cell
/// apart, and sits flush to one side of its box: right for a button row, left for a toolbar.
#[test]
fn button_rows_are_flush_with_a_cell_between() {
    let mut wrong = Vec::new();
    for scene in scenes_for_frames() {
        for size in [SMALL, LARGE] {
            let mut shell = (scene.build)(size);
            let buf = draw(&mut shell, size);
            for rects in region_lists(&shell).into_iter().flat_map(button_rows) {
                for fault in row_faults(&buf, &rects) {
                    wrong.push(format!(
                        "{} at {size:?}: buttons at y={} {fault}",
                        scene.name, rects[0].y
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_screen_renders_into_a_tiny_terminal_without_panicking() {
    for scene in scenes() {
        for size in [(0, 0), (1, 1), (20, 6)] {
            let mut shell = (scene.build)(SMALL);
            let area = Rect::new(0, 0, size.0, size.1);
            for _ in 0..2 {
                shell.poll();
                shell.render(area, &mut Buffer::empty(area));
            }
        }
    }
}
