//! the Networks, My Account and Permissions windows driven through the text view: what each
//! shows and what its buttons, tabs, fields and boxes do, with a fake gateway behind them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lanlord_app::gateway::Fake;
use lanlord_app::login::fake::FakeAuth;
use lanlord_app::login::SignedIn;
use lanlord_model::access::{Grants, Level};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyModifiers;
use ratatui::crossterm::event::{Event, KeyCode, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::tests::{click, frame, key, module, type_text};
use super::{HostList, Palette, Shell, Target};
use crate::paint::lines;

pub(super) const NETWORKS: &str = "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n";

/// a signed-in shell on the demo server whose gateway is `fake`, which the test keeps to look at.
fn server(fake: &Arc<Fake>) -> Shell {
    let mut shell = Shell::new(fake.clone(), HostList::new(None), Palette::default());
    shell.clock = Some("12:00 PM".into());
    shell.use_demo_server();
    frame(&mut shell);
    shell
}

/// draw frames until `needle` is on screen, and give the screen back.
pub(super) fn until(shell: &mut Shell, needle: &str) -> String {
    let started = Instant::now();
    loop {
        let text = frame(shell);
        if text.contains(needle) || started.elapsed() > Duration::from_secs(5) {
            assert!(text.contains(needle), "{needle} never showed:\n{text}");
            return text;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// where `needle` is on screen: its row, and the column of its middle.
pub(super) fn find(text: &str, needle: &str) -> Position {
    let (y, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle} is not on screen:\n{text}"));
    let before = line.split(needle).next().unwrap_or_default();
    let x = before.chars().count() + needle.chars().count() / 2;
    Position::new(x as u16, y as u16)
}

/// click what says `needle` on screen.
pub(super) fn press(shell: &mut Shell, needle: &str) -> String {
    let text = frame(shell);
    click(shell, find(&text, needle));
    frame(shell)
}

/// click the field in the row under the line that says `label`, and type `typed` in it.
pub(super) fn fill_below(shell: &mut Shell, label: &str, typed: &str) {
    let text = frame(shell);
    let at = find(&text, label);
    let first = text.lines().nth(usize::from(at.y)).unwrap_or_default();
    let x = first.find(label).map_or(0, |n| first[..n].chars().count()) as u16;
    click(shell, Position::new(x + 2, at.y + 1));
    type_text(shell, typed);
}

fn open(shell: &mut Shell, target: Target) {
    shell.open(target);
    frame(shell);
}

fn settings(fake: &Arc<Fake>) {
    *fake.stored.lock().unwrap() = NETWORKS.into();
}

fn count(fake: &Fake, call: &str) -> usize {
    fake.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| *c == call)
        .count()
}

/// how many calls the fake got that start with `prefix`, whatever host they name.
fn calls_starting(fake: &Fake, prefix: &str) -> usize {
    fake.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.starts_with(prefix))
        .count()
}

#[test]
fn networks_show_a_tab_per_network_with_its_connection_and_activity() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    let text = until(&mut shell, "Activity");
    for needle in [
        "External Network",
        "retronet",
        "Connection",
        "Status:",
        "Connected",
        "Interface: eth0",
        "Speed:",
        "Address:",
        "Bytes:",
        "48,211,933",
        "Packets:",
        "Refresh",
        "Details...",
        "DNS & DHCP...",
        "Properties...",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    assert!(
        !text.contains("LANlord network"),
        "the external tab has no dhcp"
    );
    // the other network's tab is a click away, and has its dhcp
    let text = press(&mut shell, "retronet");
    for needle in [
        "LANlord network",
        "DHCP range:",
        "172.16.120.100 - 172.16.120.200",
        "Clients with a lease:",
        "eth1",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    assert_eq!(count(&fake, "network-status demo@gateway"), 1);
}

#[test]
fn the_properties_box_lists_the_network_and_closes_with_ok() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    until(&mut shell, "Activity");
    press(&mut shell, "retronet");
    let text = press(&mut shell, "Properties...");
    for needle in [
        "retronet Properties",
        "Kind",
        "LANlord network",
        "MTU",
        "DHCP range",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    let text = press(&mut shell, "[ OK ]");
    assert!(!text.contains("retronet Properties"), "{text}");
    // Escape closes it too
    press(&mut shell, "Properties...");
    key(&mut shell, KeyCode::Esc);
    assert!(!frame(&mut shell).contains("retronet Properties"));
}

#[test]
fn details_opens_the_network_connection_list_and_close_ends_it() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    until(&mut shell, "Activity");
    let text = press(&mut shell, "Details...");
    for needle in [
        "Network Connection Details",
        "Property",
        "Physical Address",
        "IPv4 Address",
        "192.0.2.10",
        "IPv4 Default Gateway",
        "[ Close ]",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    let text = press(&mut shell, "[ Close ]");
    assert!(!text.contains("Physical Address"), "{text}");
}

#[test]
fn dns_and_dhcp_opens_for_a_lanlord_network_only() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    until(&mut shell, "Activity");
    let dns = module("dns");
    // the external network has none: the button does nothing
    press(&mut shell, "DNS & DHCP...");
    frame(&mut shell);
    assert!(shell.windows.iter().all(|w| w.target != dns));
    press(&mut shell, "retronet");
    press(&mut shell, "DNS & DHCP...");
    frame(&mut shell);
    assert!(shell.windows.iter().any(|w| w.target == dns));
}

#[test]
fn refresh_reads_the_networks_and_their_status_again() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    until(&mut shell, "Activity");
    until(&mut shell, "up to date");
    press(&mut shell, "[ Refresh ]");
    let started = Instant::now();
    while count(&fake, "network-status demo@gateway") < 2
        && started.elapsed() < Duration::from_secs(5)
    {
        frame(&mut shell);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(count(&fake, "network-status demo@gateway"), 2);
}

#[test]
fn a_failed_status_read_says_why_and_refresh_tries_again() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    *fake.network_error.lock().unwrap() = Some("ip is not installed".into());
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    let text = until(&mut shell, "Could not read the connection status");
    assert!(text.contains("ip is not installed"), "{text}");
    assert!(!text.contains("Activity"), "{text}");
    // Details needs a status; there is none
    let text = press(&mut shell, "Details...");
    assert!(!text.contains("Network Connection Details"), "{text}");
    // a failure is not retried on its own; Refresh does
    *fake.network_error.lock().unwrap() = None;
    until(&mut shell, "up to date");
    assert!(!frame(&mut shell).contains("Activity"));
    press(&mut shell, "[ Refresh ]");
    until(&mut shell, "Activity");
}

#[test]
fn my_account_shows_the_person_their_roles_and_linked_accounts() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::MyAccount);
    let text = until(&mut shell, "Linux account");
    for needle in [
        "alice",
        "Roles: Administrators",
        "Profile",
        "Full name",
        "Description",
        "Save profile",
        "Linked accounts",
        "Account",
        "Status",
        "active",
        "NT4 domain",
        "not available yet",
        "Change Password...",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
}

#[test]
fn the_profile_is_edited_and_saved() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::MyAccount);
    until(&mut shell, "Linux account");
    // nothing changed yet: nothing to save
    press(&mut shell, "Save profile");
    assert_eq!(count(&fake, "profile demo@gateway"), 0);
    let text = frame(&mut shell);
    let at = find(&text, "Full name");
    click(&mut shell, Position::new(at.x + 12, at.y));
    type_text(&mut shell, "Alice Example");
    let text = frame(&mut shell);
    assert!(text.contains("Alice Example"), "{text}");
    press(&mut shell, "Save profile");
    until(&mut shell, "saved");
    assert_eq!(fake.profile.lock().unwrap().full_name, "Alice Example");
}

#[test]
fn change_password_checks_the_fields_then_reports_each_linked_account() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::MyAccount);
    until(&mut shell, "Linux account");
    let text = press(&mut shell, "Change Password...");
    for needle in [
        "Old Password",
        "New Password",
        "Confirm New Password",
        "Generate",
        "Show",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    // nothing typed
    let text = press(&mut shell, "[ OK ]");
    assert!(text.contains("Type the new password."), "{text}");
    fill_below(&mut shell, "Old Password", "old-secret");
    fill_below(&mut shell, "New Password:", "new-secret");
    fill_below(&mut shell, "Confirm New Password", "other-secret");
    let text = press(&mut shell, "[ OK ]");
    assert!(text.contains("are different"), "{text}");
    // the typed text is masked until Show is pressed
    assert!(!text.contains("new-secret"), "{text}");
    fill_below(&mut shell, "Confirm New Password", "x");
    let text = press(&mut shell, "Cancel");
    assert!(
        !text.contains("Confirm New Password"),
        "Cancel closed it:\n{text}"
    );
    assert_eq!(count(&fake, "password demo@gateway"), 0);

    press(&mut shell, "Change Password...");
    fill_below(&mut shell, "Old Password", "old-secret");
    fill_below(&mut shell, "New Password:", "new-secret");
    fill_below(&mut shell, "Confirm New Password", "new-secret");
    press(&mut shell, "[ OK ]");
    let text = until(&mut shell, "Your password was changed.");
    assert!(
        text.contains("Linux account") && text.contains("changed"),
        "{text}"
    );
    assert_eq!(count(&fake, "password demo@gateway"), 1);
}

#[test]
fn a_refused_password_change_says_nothing_was_left_half_done() {
    use lanlord_model::identity::{LinkReport, LinkState};
    let fake = Arc::new(Fake::new());
    *fake.password_report.lock().unwrap() = lanlord_model::identity::PasswordReport {
        ok: false,
        links: vec![LinkReport {
            link: "Linux account".into(),
            state: LinkState::Failed("too short".into()),
        }],
    };
    let mut shell = server(&fake);
    open(&mut shell, Target::MyAccount);
    until(&mut shell, "Linux account");
    press(&mut shell, "Change Password...");
    fill_below(&mut shell, "Old Password", "old-secret");
    fill_below(&mut shell, "New Password:", "n");
    fill_below(&mut shell, "Confirm New Password", "n");
    press(&mut shell, "[ OK ]");
    let text = until(&mut shell, "was not changed");
    assert!(text.contains("failed: too short"), "{text}");
}

#[test]
fn permissions_list_users_roles_and_what_each_role_may_do() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::Permissions);
    let text = until(&mut shell, "alice");
    for needle in [
        "Users",
        "Roles",
        "ben",
        "Administrators",
        "Operators",
        "Server status",
        "Network and DHCP",
        "Manage",
        "Add user...",
        "New role...",
        "Remove role",
        "Save",
        "Reload",
        "loaded the permissions",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
}

#[test]
fn a_users_roles_are_ticked_and_saved() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::Permissions);
    until(&mut shell, "alice");
    // Save has nothing to save yet
    press(&mut shell, "Save");
    assert_eq!(count(&fake, "save-access demo@gateway"), 0);
    let text = press(&mut shell, "ben");
    assert!(text.contains("[ ] Operators"), "{text}");
    assert!(text.contains("[✓] Users"), "{text}");
    let text = press(&mut shell, "[ ] Operators");
    assert!(text.contains("[✓] Operators"), "{text}");
    // the users list shows the change before it is saved
    let ben = text.lines().find(|l| l.contains("ben")).unwrap();
    assert!(ben.contains("Users, Operators"), "{ben}");
    press(&mut shell, "Save");
    until(&mut shell, "saved");
    let members = fake.access.lock().unwrap().members.clone();
    assert!(
        members["ben"].contains(&"Operators".to_string()),
        "{members:?}"
    );
}

#[test]
fn add_user_asks_for_a_name_and_lists_the_new_user() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::Permissions);
    until(&mut shell, "alice");
    let text = press(&mut shell, "Add user...");
    assert!(text.contains("Name"), "{text}");
    // an empty name is refused, and the box stays
    let text = press(&mut shell, "[ OK ]");
    assert!(text.contains("Type a name."), "{text}");
    let at = find(&text, "Name");
    click(&mut shell, Position::new(at.x + 8, at.y + 1));
    type_text(&mut shell, "cara");
    key(&mut shell, KeyCode::Enter);
    let text = frame(&mut shell);
    assert!(!text.contains("Type a name."), "{text}");
    assert!(text.contains("cara"), "{text}");
    press(&mut shell, "Save");
    until(&mut shell, "saved");
    assert!(fake.access.lock().unwrap().members.contains_key("cara"));
}

#[test]
fn a_new_role_gets_levels_and_is_removed_again() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::Permissions);
    until(&mut shell, "alice");
    // built-in roles cannot be removed
    let before = frame(&mut shell);
    let after = press(&mut shell, "Remove role");
    assert_eq!(before, after, "Remove role is disabled on a built-in role");
    for built_in in ["Administrators", "Operators", "Users"] {
        assert!(after.contains(built_in), "{after}");
    }
    press(&mut shell, "New role...");
    type_text(&mut shell, "Auditors");
    key(&mut shell, KeyCode::Enter);
    let text = press(&mut shell, "Auditors");
    assert!(text.contains("Auditors"), "{text}");
    // a custom role has a drop-down per area: pick View for the first one
    let text = press(&mut shell, "Hidden");
    assert!(text.contains("View"), "{text}");
    press(&mut shell, "View");
    press(&mut shell, "Save");
    until(&mut shell, "saved");
    let access = fake.access.lock().unwrap().clone();
    let role = access.roles.iter().find(|r| r.name == "Auditors").unwrap();
    assert_eq!(
        role.level(lanlord_core::access::Area::ALL[0]),
        lanlord_core::access::Level::View
    );
    // and Remove role takes it away
    press(&mut shell, "Remove role");
    let text = frame(&mut shell);
    assert!(!text.contains("Auditors"), "{text}");
    press(&mut shell, "Save");
    let started = Instant::now();
    while fake
        .access
        .lock()
        .unwrap()
        .roles
        .iter()
        .any(|r| r.name == "Auditors")
        && started.elapsed() < Duration::from_secs(5)
    {
        frame(&mut shell);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fake.access.lock().unwrap().roles.is_empty());
}

fn signed_in(grants: Option<Grants>, admin: bool) -> SignedIn {
    SignedIn {
        target: "alice@gateway.example".into(),
        identity: lanlord_model::signin::Identity {
            user: "alice".into(),
            uid: 1000,
            admin,
            grants,
        },
    }
}

#[test]
fn reload_reads_again_and_signing_off_and_on_starts_the_windows_clean() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    shell.auth = Some(Arc::new(FakeAuth::default()));
    open(&mut shell, Target::Permissions);
    until(&mut shell, "alice");
    press(&mut shell, "Reload");
    until(&mut shell, "loaded the permissions");
    assert_eq!(count(&fake, "load-access demo@gateway"), 2);
    // an unsaved user and an open box do not survive a sign-off and a new sign-in
    press(&mut shell, "Add user...");
    let at = find(&frame(&mut shell), "Name");
    click(&mut shell, Position::new(at.x + 8, at.y + 1));
    type_text(&mut shell, "cara");
    key(&mut shell, KeyCode::Enter);
    press(&mut shell, "Add user...");
    assert!(shell.permissions.dialog().is_some());
    shell.log_off();
    assert!(shell.login.is_some());
    assert!(shell.permissions.dialog().is_none());
    shell.finish_login(signed_in(None, true));
    open(&mut shell, Target::Permissions);
    let text = until(&mut shell, "alice");
    assert!(!text.contains("cara"), "{text}");
    assert!(shell.permissions.dialog().is_none());
}

#[test]
fn a_read_only_account_sees_permissions_but_changes_nothing() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    shell.finish_login(signed_in(Some(Grants::all(Level::View)), false));
    open(&mut shell, Target::Permissions);
    until(&mut shell, "alice");
    let before = press(&mut shell, "ben");
    assert!(before.contains("[ ] Operators"), "{before}");
    // nothing can be ticked, added or removed, and there is nothing to save
    let after = press(&mut shell, "[ ] Operators");
    assert_eq!(before, after);
    for button in ["Add user...", "New role...", "Remove role"] {
        press(&mut shell, button);
        assert!(shell.permissions.dialog().is_none(), "{button}");
    }
    press(&mut shell, "Save");
    assert_eq!(calls_starting(&fake, "save-access"), 0);
    let text = frame(&mut shell);
    let ben = text.lines().find(|l| l.contains("ben")).unwrap();
    assert!(ben.contains("Users") && !ben.contains("Operators"), "{ben}");
}

fn small(shell: &mut Shell) -> String {
    shell.poll();
    let area = Rect::new(0, 0, 80, 24);
    let mut buf = Buffer::empty(area);
    shell.render(area, &mut buf);
    lines(&buf).join("\n")
}

#[test]
fn a_networks_page_taller_than_a_small_screen_scrolls_to_its_status_bar() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    until(&mut shell, "Activity");
    let mut text = small(&mut shell);
    let at = find(&text, "retronet");
    click(&mut shell, at);
    text = small(&mut shell);
    assert!(text.contains("LANlord network"), "{text}");
    assert!(text.contains("[ Refresh ]"), "{text}");
    // not a regression: the wheel scrolls the smallest region under it, and a list under the
    // pointer takes the wheel to move its selection. with the interface list above them, the
    // first notch brings a list under row 8, so later notches stop scrolling the page. row 4 is
    // over the page for every notch
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 40,
        row: 4,
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..3 {
        shell.on_event(Event::Mouse(wheel), Instant::now());
        small(&mut shell);
    }
    assert!(small(&mut shell).contains("up to date"));
}

fn my_server_rect(shell: &Shell) -> Rect {
    shell
        .windows
        .iter()
        .find(|w| w.target == Target::MyServer)
        .unwrap()
        .rect
}

#[test]
fn my_server_is_a_fixed_dialog_of_one_size_on_every_tab() {
    let fake = Arc::new(Fake::new());
    let mut shell = server(&fake);
    open(&mut shell, Target::MyServer);
    let text = until(&mut shell, "Processor");
    let rect = my_server_rect(&shell);
    // minimise and close, and no maximise
    let bar = text.lines().find(|l| l.contains("My Server ─")).unwrap();
    assert!(bar.contains("_ ×") && !bar.contains('□'), "{bar}");
    for needle in [
        "Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz",
        "Processors   4",
        "Memory       4096 MB, 3328 MB free",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    assert!(!text.contains("Network interfaces"), "{text}");
    // every tab fits in the same window, down to its last line
    for (tab, last) in [
        ("Advanced", "Remove LANlord from this server..."),
        ("Compatibility", "Re-check"),
        ("General", "Last applied"),
    ] {
        let text = press(&mut shell, tab);
        assert!(text.contains(last), "{tab}:\n{text}");
        assert_eq!(my_server_rect(&shell), rect, "{tab}");
    }
    // a double-click on the title bar and the button's old place do not maximise it
    let text = frame(&mut shell);
    let title = find(&text, "My Server ─");
    shell.double_click(title);
    assert_eq!(my_server_rect(&shell), rect);
    let layout = crate::paint::chrome::WindowFrame::layout_for(rect, true);
    assert_eq!(layout.maximise.width, 0);
    // the cell the maximise button has on a resizable window is not a button here
    let old = crate::paint::chrome::WindowFrame::layout_for(rect, false).maximise;
    click(&mut shell, (old.x, old.y).into());
    assert_eq!(my_server_rect(&shell), rect);
    shell.toggle_maximise(Target::MyServer);
    frame(&mut shell);
    assert_eq!(my_server_rect(&shell), rect);
}

#[test]
fn networks_list_every_interface_the_server_has() {
    let fake = Arc::new(Fake::new());
    settings(&fake);
    let mut shell = server(&fake);
    open(&mut shell, Target::Networks);
    let text = until(&mut shell, "Network interfaces");
    for needle in [
        "eth0   192.0.2.10/24   MTU 1500   Connected",
        "eth1   172.16.120.1/22   MTU 1450   Connected",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
    assert!(!text.contains("lo   127.0.0.1"), "{text}");
}
