use std::time::{Duration, Instant};

use lanlord_app::gateway::Fake;
use lanlord_app::login::fake::{id, FakeAuth};
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::*;
use crate::paint::lines;

const SIZE: Rect = Rect {
    x: 0,
    y: 0,
    width: 110,
    height: 34,
};

fn shell_with(fake: Fake) -> Shell {
    let mut shell = Shell::new(Arc::new(fake), HostList::new(None), Palette::default());
    shell.clock = Some("12:00 PM".into());
    shell
}

fn demo() -> Shell {
    let mut shell = shell_with(Fake::new());
    shell.use_demo_server();
    shell
}

fn frame(shell: &mut Shell) -> String {
    shell.poll();
    let mut buf = Buffer::empty(SIZE);
    shell.render(SIZE, &mut buf);
    lines(&buf).join("\n")
}

fn mouse(shell: &mut Shell, kind: MouseEventKind, at: Position, when: Instant) {
    let event = MouseEvent {
        kind,
        column: at.x,
        row: at.y,
        modifiers: KeyModifiers::NONE,
    };
    shell.on_event(Event::Mouse(event), when);
}

fn click(shell: &mut Shell, at: Position) {
    let now = Instant::now();
    mouse(shell, MouseEventKind::Down(MouseButton::Left), at, now);
    mouse(shell, MouseEventKind::Up(MouseButton::Left), at, now);
    frame(shell);
}

fn double_click(shell: &mut Shell, at: Position) {
    let now = Instant::now();
    for gap in [0, 100] {
        let when = now + Duration::from_millis(gap);
        mouse(shell, MouseEventKind::Down(MouseButton::Left), at, when);
        mouse(shell, MouseEventKind::Up(MouseButton::Left), at, when);
    }
    frame(shell);
}

fn key(shell: &mut Shell, code: KeyCode) {
    shell.on_event(
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
        Instant::now(),
    );
    frame(shell);
}

fn type_text(shell: &mut Shell, text: &str) {
    for c in text.chars() {
        key(shell, KeyCode::Char(c));
    }
}

fn middle(rect: Rect) -> Position {
    Position::new(rect.x + rect.width / 2, rect.y + rect.height / 2)
}

fn icon_of(shell: &Shell, target: Target) -> Rect {
    let i = shell.icons.iter().position(|t| *t == target).unwrap();
    shell.hits.icons[i]
}

fn button(shell: &Shell, owner: Owner, label_at: impl Fn(&Region) -> bool) -> Rect {
    shell
        .regions_of(owner)
        .into_iter()
        .find(|r| label_at(r))
        .expect("the region")
        .rect
}

#[test]
fn the_desktop_shows_icons_a_taskbar_and_the_clock() {
    let mut shell = demo();
    let text = frame(&mut shell);
    for label in [
        "My Server",
        "Networks",
        "Settings",
        "Console",
        "LANlord",
        "12:00 PM",
    ] {
        assert!(text.contains(label), "{label} missing:\n{text}");
    }
    // icons are small: 8 by 4 cells of art in a 13 by 7 cell, like the gui's proportions
    assert!(shell
        .hits
        .icons
        .iter()
        .all(|r| r.width <= 13 && r.height <= 7));
}

#[test]
fn the_lanlord_button_opens_the_menu_and_its_rows_open_windows() {
    let mut shell = demo();
    frame(&mut shell);
    let menu = shell.hits.taskbar.as_ref().unwrap().menu;
    click(&mut shell, middle(menu));
    let text = frame(&mut shell);
    assert!(
        text.contains("Quit LANlord") && text.contains("Server: demo@gateway"),
        "{text}"
    );
    let rows = shell.menu_state();
    let settings = rows
        .iter()
        .position(|r| matches!(r, Row::Item(i) if i.action == Action::Open(Target::Settings)))
        .unwrap();
    let (_, rects) = shell.hits.menu.clone().unwrap();
    let rect = rects.iter().find(|(_, n)| *n == settings).unwrap().0;
    click(&mut shell, middle(rect));
    assert!(shell.menu.is_none());
    assert_eq!(shell.active, Some(Target::Settings));
}

#[test]
fn double_clicking_an_icon_opens_its_window_and_the_title_bar_drags_it() {
    let mut shell = demo();
    frame(&mut shell);
    let icon = icon_of(&shell, Target::Console);
    double_click(&mut shell, middle(icon));
    let text = frame(&mut shell);
    assert_eq!(shell.active, Some(Target::Console));
    assert!(text.contains("Console"), "{text}");
    let (_, rect, layout, _) = shell.hits.windows.last().cloned().unwrap();
    let grab = Position::new(layout.title_bar.x + 3, layout.title_bar.y);
    let now = Instant::now();
    mouse(
        &mut shell,
        MouseEventKind::Down(MouseButton::Left),
        grab,
        now,
    );
    let to = Position::new(grab.x + 5, grab.y + 2);
    mouse(&mut shell, MouseEventKind::Drag(MouseButton::Left), to, now);
    mouse(&mut shell, MouseEventKind::Up(MouseButton::Left), to, now);
    let moved = shell
        .windows
        .iter()
        .find(|w| w.target == Target::Console)
        .unwrap()
        .rect;
    assert_eq!((moved.x, moved.y), (rect.x + 5, rect.y + 2));
}

#[test]
fn the_close_box_closes_and_the_taskbar_button_minimises_and_restores() {
    let mut shell = demo();
    frame(&mut shell);
    shell.open(Target::Hosts);
    frame(&mut shell);
    let task = shell.hits.taskbar.as_ref().unwrap().buttons[0];
    click(&mut shell, middle(task));
    assert!(shell.windows[0].minimized);
    click(&mut shell, middle(task));
    assert!(!shell.windows[0].minimized);
    let close = shell.hits.windows.last().unwrap().2.close;
    click(&mut shell, Position::new(close.x, close.y));
    assert!(shell.windows.is_empty());
}

#[test]
fn a_list_row_is_picked_with_the_mouse() {
    let mut shell = shell_with(Fake::new());
    let _ = shell.hosts.add("one@gw");
    let _ = shell.hosts.add("two@gw");
    frame(&mut shell);
    shell.open(Target::Hosts);
    let text = frame(&mut shell);
    assert!(text.contains("one@gw") && text.contains("two@gw"), "{text}");
    let row = button(&shell, Owner::Window(Target::Hosts), |r| {
        r.part == Part::Row(1)
    });
    click(&mut shell, middle(row));
    assert_eq!(shell.hosts.selected(), Some("two@gw"));
}

#[test]
fn a_host_is_added_through_the_dialog_with_the_keyboard() {
    let mut shell = shell_with(Fake::new());
    frame(&mut shell);
    shell.open(Target::Hosts);
    frame(&mut shell);
    let regions = shell.regions_of(Owner::Window(Target::Hosts));
    let add = regions
        .iter()
        .filter(|r| matches!(r.part, Part::Press(_)))
        .find(|r| {
            let nodes = hosts_window::view(&shell.hosts, &shell.services, false);
            view::act(
                &nodes,
                &r.path,
                Input::Press(match r.part {
                    Part::Press(n) => n,
                    _ => 0,
                }),
            ) == Some(hosts_window::Msg::Add)
        })
        .expect("an Add button")
        .rect;
    click(&mut shell, middle(add));
    let text = frame(&mut shell);
    assert!(text.contains("Add a host"), "{text}");
    type_text(&mut shell, "me@gw");
    key(&mut shell, KeyCode::Enter);
    assert!(shell.modal.is_none(), "the box closed");
    assert_eq!(shell.hosts.hosts(), ["me@gw".to_string()]);
}

#[test]
fn tab_moves_the_keyboard_through_a_windows_controls() {
    let mut shell = demo();
    frame(&mut shell);
    shell.open(Target::Hosts);
    frame(&mut shell);
    let owner = Owner::Window(Target::Hosts);
    let focusable: Vec<String> = shell
        .regions_of(owner)
        .iter()
        .filter(|r| r.focusable)
        .map(|r| r.key.clone())
        .collect();
    assert!(focusable.len() >= 2, "{focusable:?}");
    key(&mut shell, KeyCode::Tab);
    let first = shell.view_of(owner).focus.clone();
    key(&mut shell, KeyCode::Tab);
    let second = shell.view_of(owner).focus.clone();
    assert_ne!(first, second);
    shell.on_event(
        Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        Instant::now(),
    );
    assert_eq!(shell.view_of(owner).focus, first);
}

#[test]
fn the_menu_works_from_the_keyboard() {
    let mut shell = demo();
    frame(&mut shell);
    key(&mut shell, KeyCode::F(10));
    assert_eq!(shell.menu, Some(0));
    let rows = shell.menu_state();
    let quit = rows.len() - 1;
    for _ in 0..quit {
        key(&mut shell, KeyCode::Down);
    }
    key(&mut shell, KeyCode::Enter);
    assert!(shell.quit);
}

#[test]
fn the_client_and_server_titles_match_the_gui() {
    let version = env!("CARGO_PKG_VERSION");
    // like the gui, the server joins the title once someone has signed in to it
    let client = demo();
    assert_eq!(client.title(), format!("LANlord Client v{version}"));
    let fake = Fake::new();
    *fake.local.lock().unwrap() = true;
    let server = shell_with(fake);
    assert_eq!(server.title(), format!("LANlord Server v{version}"));
}

fn fake_auth() -> Arc<FakeAuth> {
    Arc::new(FakeAuth::default())
}

#[test]
fn client_mode_starts_at_the_logon_box() {
    let mut shell = shell_with(Fake::new());
    shell.require_login(fake_auth());
    let text = frame(&mut shell);
    assert!(shell.login.is_some());
    assert!(text.contains("Logon Information"), "{text}");
    assert!(
        !text.contains("12:00 PM"),
        "no taskbar before the logon:\n{text}"
    );
    assert!(shell.hits.icons.is_empty());
}

#[test]
fn server_mode_skips_the_logon_box() {
    let fake = Fake::new();
    *fake.local.lock().unwrap() = true;
    let mut shell = shell_with(fake);
    shell.require_login(fake_auth());
    let text = frame(&mut shell);
    assert!(shell.login.is_none());
    assert!(
        text.contains("12:00 PM") && text.contains("My Server"),
        "{text}"
    );
}

#[test]
fn a_demo_sign_in_reaches_the_desktop() {
    let mut shell = shell_with(Fake::new());
    let auth = fake_auth();
    auth.push(Ok(id("demo", 1000)));
    shell.require_login(auth.clone());
    shell.use_demo_server();
    frame(&mut shell);
    // the agent is tried first, so the box only needs the server: user and host
    let login = shell.login.as_mut().unwrap();
    login.user = "demo".into();
    login.host = "gateway".into();
    frame(&mut shell);
    let ok = shell
        .regions_of(Owner::Dialog)
        .into_iter()
        .filter(|r| matches!(r.part, Part::Press(_)))
        .max_by_key(|r| (r.rect.y, r.rect.x))
        .map(|r| r.rect);
    // press the box's default button with Enter in its first field
    key(&mut shell, KeyCode::Enter);
    let started = Instant::now();
    while shell.signed_in.is_none() && started.elapsed() < Duration::from_secs(5) {
        if shell.login.as_ref().is_some_and(|l| !l.busy()) && auth.calls().is_empty() {
            if let Some(ok) = ok {
                click(&mut shell, middle(ok));
            }
        }
        frame(&mut shell);
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = frame(&mut shell);
    assert!(shell.signed_in.is_some(), "{:?}\n{text}", auth.calls());
    assert!(shell.login.is_none() && shell.admin_setup.is_none());
    assert!(
        text.contains("12:00 PM") && text.contains("My Server"),
        "{text}"
    );
    assert_eq!(
        shell.title(),
        format!(
            "LANlord Client v{} - demo@gateway",
            env!("CARGO_PKG_VERSION")
        )
    );
}

#[test]
fn root_signs_in_to_the_administrator_screen_first() {
    let mut shell = shell_with(Fake::new());
    let auth = fake_auth();
    auth.push(Ok(id("root", 0)));
    shell.require_login(auth.clone());
    let login = shell.login.as_mut().unwrap();
    login.user = "root".into();
    login.host = "gateway".into();
    login.submit();
    let started = Instant::now();
    while shell.signed_in.is_none() && started.elapsed() < Duration::from_secs(5) {
        frame(&mut shell);
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = frame(&mut shell);
    assert!(shell.admin_setup.is_some(), "{text}");
    assert!(!text.contains("12:00 PM"), "{text}");
}

#[test]
fn windows_not_on_the_layout_tree_yet_say_so() {
    let mut shell = demo();
    frame(&mut shell);
    shell.open(Target::Networks);
    let text = frame(&mut shell);
    assert!(text.contains("not available in the text view"), "{text}");
}

#[test]
fn my_server_draws_from_the_shared_tree() {
    let mut shell = demo();
    frame(&mut shell);
    shell.open(Target::MyServer);
    let text = frame(&mut shell);
    assert!(
        text.contains("General") && text.contains("Advanced"),
        "{text}"
    );
}

fn module(id: &str) -> Target {
    Target::Module(MODULES.iter().position(|m| m.id == id).unwrap())
}

#[test]
fn the_nt4_window_switches_tabs_and_dns_and_commands_draw_from_their_trees() {
    let mut shell = demo();
    frame(&mut shell);
    let nt4 = module("nt4-domain");
    shell.open(nt4);
    let text = frame(&mut shell);
    assert!(
        text.contains("Logins") && text.contains("Logon scripts"),
        "{text}"
    );
    let logins = button(&shell, Owner::Window(nt4), |r| r.part == Part::Tab(1));
    click(&mut shell, middle(logins));
    assert_eq!(shell.nt4_tab, 1);
    for id in ["dns", "remote-commands"] {
        shell.open(module(id));
        let text = frame(&mut shell);
        assert!(
            !text.contains("not available in the text view"),
            "{id}:\n{text}"
        );
    }
}
