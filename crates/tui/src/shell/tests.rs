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

pub(super) fn shell_with(fake: Fake) -> Shell {
    let mut shell = Shell::new(Arc::new(fake), HostList::new(None), Palette::default());
    shell.clock = Some("12:00 PM".into());
    shell
}

fn demo() -> Shell {
    let mut shell = shell_with(Fake::new());
    shell.use_demo_server();
    shell
}

pub(super) fn frame(shell: &mut Shell) -> String {
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

pub(super) fn click(shell: &mut Shell, at: Position) {
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

pub(super) fn key(shell: &mut Shell, code: KeyCode) {
    shell.on_event(
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
        Instant::now(),
    );
    frame(shell);
}

pub(super) fn type_text(shell: &mut Shell, text: &str) {
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
    // the menu grows once the server's components are read
    let started = Instant::now();
    while shell.enabled().is_none() && started.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
        frame(&mut shell);
    }
    key(&mut shell, KeyCode::F(10));
    let rows = shell.menu_state();
    assert_eq!(shell.menu.as_ref(), menu_actions(&rows).next());
    let quit = menu_actions(&rows).count() - 1;
    for _ in 0..quit {
        key(&mut shell, KeyCode::Down);
    }
    key(&mut shell, KeyCode::Enter);
    assert!(shell.quit);
}

#[test]
fn the_menu_highlight_stays_on_its_item_when_the_components_arrive() {
    // no frame has run yet, so the components have not been asked for
    let mut shell = demo();
    assert!(shell.enabled().is_none());
    shell.open_menu();
    // quit, which is there before and after the rows change
    shell.menu = Some(Action::Quit);
    let before = shell.menu_state().len();
    let started = Instant::now();
    while shell.enabled().is_none() && started.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
        frame(&mut shell);
    }
    assert!(shell.enabled().is_some(), "the components arrive");
    assert_eq!(shell.menu, Some(Action::Quit));
    let rows = shell.menu_state();
    assert!(rows.len() > before, "the menu grew");
    assert_eq!(menu_row_of(&rows, &Action::Quit), rows.len() - 1);
    // Enter picks that item, not whatever row the old index points at now
    key(&mut shell, KeyCode::Enter);
    assert!(shell.quit);
}

#[test]
fn windows_are_fitted_again_only_when_something_could_have_changed() {
    // with no server chosen nothing runs in the background, so no task finishes under this test
    let mut shell = shell_with(Fake::new());
    let add = |shell: &mut Shell, target| {
        shell.windows.push(Win {
            target,
            rect: Rect::new(2, 1, 10, 3),
            minimized: false,
            restore: None,
        });
    };
    add(&mut shell, Target::Console);
    frame(&mut shell);
    let fitted = shell.windows[0].rect;
    // an idle frame leaves a window where it is, even one that was moved out of fit
    shell.windows[0].rect.height = 3;
    frame(&mut shell);
    assert_eq!(shell.windows[0].rect.height, 3);
    // a key press may have changed what it shows
    key(&mut shell, KeyCode::Down);
    frame(&mut shell);
    assert_eq!(shell.windows[0].rect, fitted);
    // so does a new size for the terminal
    shell.windows[0].rect.height = 3;
    let mut buf = Buffer::empty(Rect::new(0, 0, SIZE.width, SIZE.height + 5));
    shell.render(buf.area, &mut buf);
    assert_ne!(shell.windows[0].rect.height, 3);
    // and a new window
    shell.windows[0].rect.height = 3;
    add(&mut shell, Target::Progress);
    frame(&mut shell);
    assert_ne!(shell.windows[0].rect.height, 3);
}

#[test]
fn the_menu_highlight_falls_back_to_the_first_row_when_its_item_is_gone() {
    let shell = demo();
    let rows = shell.menu_state();
    assert_eq!(menu_row_of(&rows, &Action::Rebuild), 0);
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
    assert!(text.contains("Login Information"), "{text}");
    assert!(
        !text.contains("12:00 PM"),
        "no taskbar before the logon:\n{text}"
    );
    assert!(shell.hits.icons.is_empty());
}

/// the logon box on a machine that offers "This server": the choice is there, the box still
/// fits the 80x24 screen, and picking the other server brings the ssh fields back.
#[test]
fn the_logon_box_with_this_server_fits_the_small_screen_in_both_states() {
    let auth = fake_auth();
    *auth.here.lock().unwrap() = Some("ann@this-server.localhost".into());
    let mut shell = shell_with(Fake::new());
    shell.require_login(auth);
    let here = frame(&mut shell);
    assert!(here.contains("Sign in to:"), "{here}");
    assert!(here.contains("This server (ann)"), "{here}");
    assert!(here.contains("[ OK ]"), "{here}");
    shell
        .login
        .as_mut()
        .unwrap()
        .update(login::Msg::Where(1), &[]);
    let other = frame(&mut shell);
    for part in [
        "Sign in to:",
        "Another server",
        "Username:",
        "Hostname:",
        "[ OK ]",
    ] {
        assert!(other.contains(part), "{part} missing:\n{other}");
    }
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

pub(super) fn module(id: &str) -> Target {
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
        let tab = if id == "dns" {
            "Static leases"
        } else {
            "Run on the gateway"
        };
        assert!(text.contains(tab), "{id}:\n{text}");
    }
}

fn settle_clock(shell: &mut Shell) {
    // the Date/Time window reads the server once; give the job a moment, drawing as we go
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        let text = frame(shell);
        if text.contains("Date & Time") && !text.contains("Reading the server") {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// a demo server whose clock can be set by hand (automatic synchronization off).
fn editable_clock() -> Shell {
    let fake = Fake::new();
    fake.clock.lock().unwrap().ntp = false;
    let mut shell = shell_with(fake);
    shell.use_demo_server();
    frame(&mut shell);
    shell.open(Target::DateTime);
    settle_clock(&mut shell);
    shell
}

const DATE_TIME: Owner = Owner::Window(Target::DateTime);

#[test]
fn date_time_is_a_tree_window_with_spin_boxes_and_a_calendar() {
    let mut shell = editable_clock();
    let text = frame(&mut shell);
    for needle in [
        "Date & Time",
        "Time Zone",
        "Internet Time",
        "Date",
        "Time",
        "OK",
        "Cancel",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    let regions = shell.regions_of(DATE_TIME);
    assert_eq!(
        regions
            .iter()
            .filter(|r| matches!(r.part, Part::Day(_)))
            .count(),
        42,
        "a month grid"
    );
    // year, hour, minute, second and am/pm each have an up arrow
    assert_eq!(
        regions.iter().filter(|r| r.part == Part::Spin(1)).count(),
        5
    );
    // the digital clock prints hh:mm:ss
    assert!(
        text.lines().any(|l| l.matches(':').count() >= 2),
        "a hh:mm:ss clock:\n{text}"
    );
}

#[test]
fn a_read_only_clock_takes_no_clicks_on_its_calendar_or_spin_boxes() {
    let mut shell = demo();
    frame(&mut shell);
    shell.open(Target::DateTime);
    settle_clock(&mut shell);
    let text = frame(&mut shell);
    assert!(text.contains("Automatic synchronization is on"), "{text}");
    let regions = shell.regions_of(DATE_TIME);
    assert!(regions
        .iter()
        .all(|r| !matches!(r.part, Part::Day(_) | Part::Spin(_))));
}

fn up_arrow(shell: &Shell, row: usize) -> Rect {
    let mut ups: Vec<Rect> = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .filter(|r| r.part == Part::Spin(1))
        .map(|r| r.rect)
        .collect();
    ups.sort_by_key(|r| (r.y, r.x));
    ups[row]
}

/// the text of the spin box whose up arrow is `up`: the cells just left of it.
fn spin_text(shell: &mut Shell, up: Rect) -> String {
    let text = frame(shell);
    let line = text.lines().nth(usize::from(up.y)).unwrap_or_default();
    let before: String = line.chars().take(usize::from(up.x)).collect();
    before
        .trim()
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// the selected day: the day cell drawn with the selection colour.
fn selected_day(shell: &mut Shell) -> Option<usize> {
    frame(shell);
    let mut buf = Buffer::empty(SIZE);
    shell.render(SIZE, &mut buf);
    let white = Palette::default().color((255, 255, 255));
    let regions = shell.regions_of(DATE_TIME);
    (0..42).find(|n| {
        regions
            .iter()
            .find(|r| r.part == Part::Day(*n))
            .is_some_and(|r| {
                buf[(r.rect.x + 1, r.rect.y)].fg == white
                    && buf[(r.rect.x + 1, r.rect.y)].bg != white
            })
    })
}

#[test]
fn clicking_a_spin_arrow_steps_the_hour_and_apply_lights_up() {
    let mut shell = editable_clock();
    let hour_up = up_arrow(&shell, 1);
    let before = spin_text(&mut shell, hour_up);
    click(&mut shell, middle(hour_up));
    let hour_up = up_arrow(&shell, 1);
    let after = spin_text(&mut shell, hour_up);
    assert_ne!(before, after, "the hour moved from {before}");
    let buttons = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .filter(|r| matches!(r.part, Part::Press(_)))
        .count();
    assert_eq!(buttons, 3, "Apply lights up once the hour changed");
}

#[test]
fn the_wheel_does_nothing_behind_an_open_menu() {
    let mut shell = editable_clock();
    let minute_up = up_arrow(&shell, 2);
    let start = spin_text(&mut shell, minute_up);
    let over = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .find(|r| r.part == Part::Spin(0) && r.rect.y == minute_up.y)
        .map(|r| r.rect)
        .expect("a spin box");
    shell.open_menu();
    mouse(
        &mut shell,
        MouseEventKind::ScrollUp,
        middle(over),
        Instant::now(),
    );
    shell.menu = None;
    let minute_up = up_arrow(&shell, 2);
    assert_eq!(start, spin_text(&mut shell, minute_up));
}

#[test]
fn the_wheel_does_nothing_behind_an_open_dialog() {
    let mut shell = editable_clock();
    let minute_up = up_arrow(&shell, 2);
    let start = spin_text(&mut shell, minute_up);
    let over = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .find(|r| r.part == Part::Spin(0) && r.rect.y == minute_up.y)
        .map(|r| r.rect)
        .expect("a spin box");
    shell.do_action(Action::About);
    assert!(shell.modal.is_some());
    mouse(
        &mut shell,
        MouseEventKind::ScrollUp,
        middle(over),
        Instant::now(),
    );
    shell.modal = None;
    let minute_up = up_arrow(&shell, 2);
    assert_eq!(start, spin_text(&mut shell, minute_up));
}

#[test]
fn f10_opens_no_menu_at_the_logon_box() {
    let mut shell = shell_with(Fake::new());
    shell.require_login(fake_auth());
    frame(&mut shell);
    key(&mut shell, KeyCode::F(10));
    assert!(shell.menu.is_none());
    assert!(shell.login.is_some());
}

#[test]
fn f10_opens_no_menu_on_the_administrator_screen() {
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
    frame(&mut shell);
    assert!(shell.admin_setup.is_some());
    key(&mut shell, KeyCode::F(10));
    assert!(shell.menu.is_none());
}

#[test]
fn the_wheel_and_arrow_keys_step_a_spin_box() {
    let mut shell = editable_clock();
    let minute_up = up_arrow(&shell, 2);
    let start = spin_text(&mut shell, minute_up);
    // the wheel over the box steps it: up counts up
    let boxes: Vec<Rect> = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .filter(|r| r.part == Part::Spin(0) && r.rect.y == minute_up.y)
        .map(|r| r.rect)
        .collect();
    let over = boxes
        .iter()
        .find(|r| r.right() >= minute_up.x && r.x <= minute_up.x)
        .copied()
        .expect("the minute box");
    mouse(
        &mut shell,
        MouseEventKind::ScrollUp,
        middle(over),
        Instant::now(),
    );
    let minute_up = up_arrow(&shell, 2);
    let wheeled = spin_text(&mut shell, minute_up);
    assert_ne!(start, wheeled, "the wheel stepped the minute from {start}");
    // Tab onto a spin box, then Up steps it
    for _ in 0..60 {
        key(&mut shell, KeyCode::Tab);
        if shell
            .view_of(DATE_TIME)
            .focus
            .as_deref()
            .is_some_and(|f| f.contains("Spin"))
        {
            break;
        }
    }
    let focus = shell.view_of(DATE_TIME).focus.clone().unwrap_or_default();
    assert!(
        focus.contains("Spin"),
        "the keyboard reached a spin box: {focus}"
    );
    let at = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .find(|r| r.key == focus)
        .expect("the focused box")
        .rect;
    let line = frame(&mut shell)
        .lines()
        .nth(usize::from(at.y))
        .unwrap()
        .to_string();
    key(&mut shell, KeyCode::Up);
    let moved = frame(&mut shell)
        .lines()
        .nth(usize::from(at.y))
        .unwrap()
        .to_string();
    assert_ne!(line, moved, "Up stepped the focused spin box");
}

#[test]
fn clicking_a_day_picks_it_and_the_arrow_keys_move_the_selection() {
    let mut shell = editable_clock();
    frame(&mut shell);
    let picked = selected_day(&mut shell);
    let target = if picked == Some(10) { 11 } else { 10 };
    let day = shell
        .regions_of(DATE_TIME)
        .into_iter()
        .find(|r| r.part == Part::Day(target))
        .expect("a day")
        .rect;
    click(&mut shell, middle(day));
    assert_eq!(
        selected_day(&mut shell),
        Some(target),
        "the clicked day is selected"
    );
    // the calendar takes the keyboard: Tab onto it, then Right moves the selection a day
    for _ in 0..60 {
        key(&mut shell, KeyCode::Tab);
        if shell
            .view_of(DATE_TIME)
            .focus
            .as_deref()
            .is_some_and(|f| f.contains("Days"))
        {
            break;
        }
    }
    key(&mut shell, KeyCode::Right);
    assert_eq!(
        selected_day(&mut shell),
        Some(target + 1),
        "Right moved it a day"
    );
    key(&mut shell, KeyCode::Down);
    assert_eq!(
        selected_day(&mut shell),
        Some(target + 8),
        "Down moved it a week"
    );
    // the taskbar clock still shows a time from the feed
    let text = frame(&mut shell);
    assert!(text.contains("AM") || text.contains("PM"), "{text}");
}

#[test]
fn cancel_closes_the_date_time_window() {
    let mut shell = editable_clock();
    let regions = shell.regions_of(DATE_TIME);
    let mut buttons: Vec<&Region> = regions
        .iter()
        .filter(|r| matches!(r.part, Part::Press(_)))
        .collect();
    // the row is drawn Apply, Cancel, OK from the right: sort left to right
    buttons.sort_by_key(|r| r.rect.x);
    // Apply is greyed out until something changes, so it takes no clicks: OK and Cancel
    assert_eq!(buttons.len(), 2, "Cancel and OK are live");
    let cancel = buttons[0].rect;
    click(&mut shell, middle(cancel));
    frame(&mut shell);
    assert!(
        shell.windows.iter().all(|w| w.target != Target::DateTime),
        "Cancel closed it"
    );
}

fn settle_components(shell: &mut Shell) -> String {
    // the window reads the server's settings once; give the job a moment, drawing as we go
    let started = Instant::now();
    let mut text = frame(shell);
    while started.elapsed() < Duration::from_secs(3) && !text.contains("Next >") {
        std::thread::sleep(Duration::from_millis(10));
        text = frame(shell);
    }
    text
}

#[test]
fn the_components_window_opens_as_a_wizard_with_a_check_list() {
    let mut shell = demo();
    frame(&mut shell);
    shell.open(Target::Components);
    let text = settle_components(&mut shell);
    for needle in ["LANlord Components", "[x] ", "Next >", "Cancel"] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    assert!(shell.windows.iter().any(|w| w.target == Target::Components));
    // Cancel closes it
    let regions = shell
        .hits
        .windows
        .iter()
        .find(|w| w.0 == Target::Components)
        .map(|w| w.3.clone())
        .expect("the window was drawn");
    let cancel = regions
        .iter()
        .filter(|r| matches!(r.part, Part::Press(_)))
        .max_by_key(|r| r.rect.x)
        .expect("buttons");
    click(&mut shell, middle(cancel.rect));
    frame(&mut shell);
    assert!(shell.windows.iter().all(|w| w.target != Target::Components));
}

#[test]
fn the_welcome_window_opens_after_logon_only_while_its_box_is_ticked() {
    let dir = std::env::temp_dir().join(format!("lanlord-tui-welcome-{}", std::process::id()));
    let file = dir.join("prefs");
    let signed_in = |user: &str| lanlord_app::login::SignedIn {
        target: format!("{user}@gw.lan"),
        identity: lanlord_core::signin::Identity {
            user: user.into(),
            uid: 1000,
            admin: true,
            grants: None,
        },
    };
    let mut shell = demo();
    shell.set_prefs(Some(file.clone()));
    shell.finish_login(signed_in("alice"));
    let text = frame(&mut shell);
    assert!(shell.windows.iter().any(|w| w.target == Target::Welcome));
    for needle in [
        "Networks",
        "Open Networks",
        "Show this Welcome screen",
        "Close",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    // clearing the box is remembered for that account only
    shell.welcome.update(lanlord_app::welcome::Msg::Toggle, &[]);
    for (user, shown) in [("alice", false), ("bob", true)] {
        let mut shell = demo();
        shell.set_prefs(Some(file.clone()));
        shell.finish_login(signed_in(user));
        assert_eq!(
            shell.windows.iter().any(|w| w.target == Target::Welcome),
            shown,
            "{user}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

const COMPAT_SETTINGS: &str = "docker: {}\ndns: {}\n";

/// a shell on a server whose probe says `probe`, with docker and dns switched on, drawn until
/// the report is in.
fn probed(probe: &str) -> Shell {
    let fake = Fake::new();
    *fake.probe.lock().unwrap() = probe.into();
    *fake.stored.lock().unwrap() = COMPAT_SETTINGS.into();
    let mut shell = shell_with(fake);
    shell.use_demo_server();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        frame(&mut shell);
        let host = shell.host().unwrap_or_default();
        if shell.services.compat().report(&host).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    frame(&mut shell);
    shell
}

const LXC_WITHOUT_FEATURES: &str =
    "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=no\nkeyctl=no\n";

#[test]
fn the_taskbar_warns_when_a_switched_on_component_needs_what_fails() {
    let mut shell = probed(LXC_WITHOUT_FEATURES);
    assert!(shell.compat_alert());
    let text = frame(&mut shell);
    assert!(text.contains("! Compatibility"), "{text}");
    let alert = shell
        .hits
        .taskbar
        .as_ref()
        .unwrap()
        .alert
        .expect("a hit area");
    click(&mut shell, middle(alert));
    let text = frame(&mut shell);
    assert!(shell.windows.iter().any(|w| w.target == Target::MyServer));
    assert_eq!(shell.my_server.tab(), lanlord_app::my_server::COMPATIBILITY);
    assert!(text.contains("Needed for"), "{text}");
}

#[test]
fn f8_opens_the_compatibility_tab_and_a_healthy_server_has_no_warning() {
    let mut shell = probed(LXC_WITHOUT_FEATURES);
    key(&mut shell, KeyCode::F(8));
    frame(&mut shell);
    assert_eq!(shell.my_server.tab(), lanlord_app::my_server::COMPATIBILITY);

    let mut shell =
        probed("virt_container=lxc\nvirt_vm=none\nuid_map=0 0 4294967295\nnesting=yes\n");
    assert!(!shell.compat_alert());
    assert!(!frame(&mut shell).contains("! Compatibility"));
    key(&mut shell, KeyCode::F(8));
    frame(&mut shell);
    assert!(shell.windows.iter().all(|w| w.target != Target::MyServer));
}

#[test]
fn an_apply_opens_its_progress_dialog_and_close_ends_it() {
    use lanlord_app::apply_progress::{ApplyEvent, Msg, Stats};
    let mut shell = demo();
    frame(&mut shell);
    shell.console.apply.event(&ApplyEvent::Total { n: 2 });
    shell.console.apply.event(&ApplyEvent::TaskStart {
        name: "write the config".into(),
    });
    let text = frame(&mut shell);
    assert!(text.contains("Task 1 of 2: write the config"), "{text}");
    assert!(shell.windows.iter().any(|w| w.target == Target::Progress));
    let finished = ApplyEvent::Finished {
        stats: Stats::default(),
    };
    shell.console.apply.event(&finished);
    frame(&mut shell);
    shell.console.apply.update(Msg::Close);
    frame(&mut shell);
    frame(&mut shell);
    assert!(!shell.windows.iter().any(|w| w.target == Target::Progress));
}
