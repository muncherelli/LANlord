//! drawing: a classic gray-on-teal desktop with navy title bars.

use lanlord_core::log::Kind;
use lanlord_core::modules::{Module, Status, MODULES};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, Mode, UsersState};

const DESKTOP: Color = Color::Rgb(0, 128, 128);
const NAVY: Color = Color::Rgb(0, 0, 128);
const GRAY: Color = Color::Rgb(192, 192, 192);

fn window(title: String) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Double)
        .title(title)
        .style(Style::new().bg(GRAY).fg(Color::Black))
}

/// width and height of one desktop icon cell.
const CELL_W: u16 = 13;
const CELL_H: u16 = 4;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::new().bg(DESKTOP)), area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    let title = Style::new()
        .bg(NAVY)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(" LANlord ").style(title), rows[0]);

    let body = Layout::horizontal([Constraint::Length(28), Constraint::Min(10)]).split(rows[1]);
    draw_hosts(frame, app, body[0]);

    let columns = ((body[1].width.saturating_sub(2)) / CELL_W).max(1) as usize;
    let icon_rows = MODULES.len().div_ceil(columns) as u16;
    let right = Layout::vertical([
        Constraint::Length(icon_rows * CELL_H + 2),
        Constraint::Min(3),
    ])
    .split(body[1]);
    draw_desktop(frame, app, right[0], columns);
    draw_console(frame, app, right[1]);

    draw_input(frame, app, rows[2]);

    let hint = match app.mode {
        Mode::Normal => {
            " tab pane   arrows select   enter open   b bootstrap   p apply   a add host   q quit "
        }
        Mode::Command => " enter run   esc back ",
        Mode::AddHost => " enter save   esc cancel ",
        Mode::Password => " enter send   esc cancel ",
        Mode::Window if app.window.is_some_and(|m| m.id == "nt4-domain") => {
            " n new   p password   d delete   r refresh   a apply   esc close "
        }
        Mode::Window => " enter apply this module   esc close ",
        Mode::UserName | Mode::UserPassword | Mode::UserRepeat => " enter next   esc cancel ",
        Mode::UserDelete => " y delete   any other key cancels ",
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::new().bg(GRAY).fg(Color::Black)),
        rows[3],
    );

    if let Some(module) = app.window {
        draw_window(frame, app, module, area);
    }
}

fn focus_title(label: &str, focused: bool) -> String {
    if focused {
        format!(" {label} * ")
    } else {
        format!(" {label} ")
    }
}

fn draw_hosts(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if app.hosts.is_empty() {
        vec![ListItem::new("(none - press a)")]
    } else {
        app.hosts
            .iter()
            .map(|h| ListItem::new(h.as_str()))
            .collect()
    };
    let list = List::new(items)
        .block(window(focus_title("Hosts", app.focus == Focus::Hosts)))
        .highlight_style(Style::new().bg(NAVY).fg(Color::White));
    let mut state = ListState::default();
    if !app.hosts.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_desktop(frame: &mut Frame, app: &App, area: Rect, columns: usize) {
    let focused = app.focus == Focus::Desktop;
    let block = window(focus_title("Desktop", focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    for (i, module) in MODULES.iter().enumerate() {
        let cell = Rect::new(
            inner.x + (i % columns) as u16 * CELL_W,
            inner.y + (i / columns) as u16 * CELL_H,
            CELL_W.min(inner.width),
            CELL_H,
        );
        if cell.right() > inner.right() || cell.bottom() > inner.bottom() {
            continue;
        }
        let style = if focused && i == app.icon {
            Style::new().bg(NAVY).fg(Color::White)
        } else {
            Style::new().bg(GRAY).fg(Color::Black)
        };
        let mut lines: Vec<Line> = module
            .icon
            .iter()
            .map(|art| Line::from(*art).centered())
            .collect();
        lines.push(Line::from(module.name).centered());
        frame.render_widget(Paragraph::new(lines).style(style), cell);
    }
}

fn draw_console(frame: &mut Frame, app: &App, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let skip = app.console.len().saturating_sub(height);
    let lines: Vec<Line> = app
        .console
        .iter()
        .skip(skip)
        .map(|l| {
            let color = match l.kind {
                Kind::Out => Color::Gray,
                Kind::Err => Color::LightRed,
                Kind::Info => Color::LightCyan,
                Kind::Debug => Color::DarkGray,
            };
            Line::from(Span::styled(l.text.as_str(), Style::new().fg(color)))
        })
        .collect();
    let console = Paragraph::new(lines).block(
        Block::bordered()
            .border_type(BorderType::Double)
            .title(" Console ")
            .style(Style::new().bg(Color::Black).fg(Color::Gray)),
    );
    frame.render_widget(console, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let (label, prompt) = match app.mode {
        Mode::AddHost => (" Add host (user@host) ".to_string(), "> "),
        Mode::Password => (
            format!(
                " {} ",
                app.password
                    .as_ref()
                    .map_or("Password", |p| p.prompt.as_str())
            ),
            "> ",
        ),
        Mode::UserName => (" New user name ".to_string(), "> "),
        Mode::UserPassword => (
            format!(" Password for {} ", app.pending_name().unwrap_or("?")),
            "> ",
        ),
        Mode::UserRepeat => (" Repeat the password ".to_string(), "> "),
        Mode::UserDelete => (
            format!(" Delete {}? (y/n) ", app.pending_name().unwrap_or("?")),
            "  ",
        ),
        _ => (
            format!(" Command on {} ", app.host().unwrap_or("-")),
            if app.mode == Mode::Command {
                "> "
            } else {
                "  "
            },
        ),
    };
    // never show a password
    let shown = if matches!(
        app.mode,
        Mode::Password | Mode::UserPassword | Mode::UserRepeat
    ) {
        "*".repeat(app.input.chars().count())
    } else {
        app.input.clone()
    };
    let input = Paragraph::new(format!("{prompt}{shown}")).block(window(label));
    frame.render_widget(input, area);
    if matches!(
        app.mode,
        Mode::Command
            | Mode::AddHost
            | Mode::Password
            | Mode::UserName
            | Mode::UserPassword
            | Mode::UserRepeat
    ) {
        let x = area.x + 1 + (prompt.len() + app.input.chars().count()) as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(2)), area.y + 1));
    }
}

/// a module's window, centered over everything.
fn draw_window(frame: &mut Frame, app: &App, module: &Module, area: Rect) {
    let users = module.id == "nt4-domain";
    let width = area.width.min(64);
    let height = area.height.min(if users { 24 } else { 16 });
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let status = match module.status {
        Status::Available => "available",
        Status::Planned => "planned - not built yet",
    };
    let mut lines = vec![
        Line::from(module.summary),
        Line::from(""),
        Line::from(format!("Status: {status}")),
        Line::from(""),
        Line::from("Settings (in your settings file):"),
    ];
    lines.extend(module.example.lines().map(|l| Line::from(format!("  {l}"))));
    if users {
        lines.push(Line::from(""));
        lines.push(Line::from("Logins:"));
        match &app.users {
            UsersState::Idle => {}
            UsersState::Loading => lines.push(Line::from("  working...")),
            UsersState::Failed(err) => lines.push(Line::from(format!("  could not load: {err}"))),
            UsersState::Loaded(list) if list.is_empty() => {
                lines.push(Line::from("  (none - press n to add one)"));
            }
            UsersState::Loaded(list) => {
                for (i, user) in list.iter().enumerate() {
                    let style = if i == app.user_selected {
                        Style::new().bg(NAVY).fg(Color::White)
                    } else {
                        Style::new()
                    };
                    lines.push(Line::from(Span::styled(format!("  {}", user.name), style)));
                }
            }
        }
    }
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(window(format!(" {} ", module.name)));
    frame.render_widget(body, rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_core::log::Line as LogLine;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::sync::mpsc;

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn app() -> App {
        let (_tx, rx) = mpsc::channel();
        App::new(None, rx, Box::new(|_, _| {}))
    }

    #[test]
    fn empty_screen_prompts_for_a_host() {
        let text = render(&app());
        assert!(text.contains("LANlord"));
        assert!(text.contains("(none - press a)"));
        assert!(text.contains("Console"));
        assert!(text.contains("q quit"));
        assert!(text.contains("enter open"));
    }

    #[test]
    fn shows_hosts_and_the_tail_of_the_console() {
        let mut app = app();
        app.hosts = vec!["me@gw".into()];
        for i in 0..40 {
            app.console.push_back(LogLine {
                kind: Kind::Out,
                text: format!("line-{i}"),
            });
        }
        let text = render(&app);
        assert!(text.contains("me@gw"));
        assert!(text.contains("Command on me@gw"));
        assert!(text.contains("line-39"));
        assert!(!text.contains("line-0 "));
    }

    #[test]
    fn passwords_are_masked() {
        let (reply, _answer) = mpsc::channel();
        let mut app = app();
        app.mode = Mode::Password;
        app.password = Some(crate::app::PasswordRequest {
            prompt: "sudo password for me".into(),
            reply,
        });
        app.input = "hunter2".into();
        let text = render(&app);
        assert!(text.contains("sudo password for me"));
        assert!(text.contains("> *******"));
        assert!(!text.contains("hunter2"));
        assert!(text.contains("esc cancel"));
    }

    #[test]
    fn the_desktop_shows_every_module_icon() {
        let text = render(&app());
        for module in MODULES {
            assert!(text.contains(module.name), "{}", module.name);
        }
        assert!(text.contains("Desktop"));
    }

    #[test]
    fn an_open_window_shows_the_module() {
        let mut app = app();
        app.window = MODULES.iter().find(|m| m.id == "remote-commands");
        app.mode = Mode::Window;
        let text = render(&app);
        assert!(text.contains("Status: available"));
        assert!(text.contains("Run commands on the gateway"));
        assert!(text.contains("remote-commands:"));
        assert!(text.contains("esc close"));
        app.window = MODULES.iter().find(|m| m.id == "nt4-domain");
        let text = render(&app);
        assert!(text.contains("Status: available"));
        assert!(text.contains("name: RETRONET"));
    }

    #[test]
    fn a_narrow_terminal_wraps_the_icons() {
        let mut terminal = Terminal::new(TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| draw(f, &app())).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Remote Cmds"));
    }

    #[test]
    fn typing_modes_show_the_prompt_and_hints() {
        let mut app = app();
        app.hosts = vec!["me@gw".into()];
        app.mode = Mode::Command;
        app.input = "uptime".into();
        let text = render(&app);
        assert!(text.contains("> uptime"));
        assert!(text.contains("esc back"));
        app.mode = Mode::AddHost;
        let text = render(&app);
        assert!(text.contains("Add host"));
        assert!(text.contains("esc cancel"));
    }

    #[test]
    fn the_nt4_window_lists_the_logins() {
        use crate::app::UsersState;
        use lanlord_core::users::User;
        let mut app = app();
        app.window = MODULES.iter().find(|m| m.id == "nt4-domain");
        app.mode = Mode::Window;
        app.users = UsersState::Loading;
        assert!(render(&app).contains("working..."));
        app.users = UsersState::Failed("no settings on gw".into());
        assert!(render(&app).contains("could not load: no settings on gw"));
        app.users = UsersState::Loaded(Vec::new());
        assert!(render(&app).contains("press n to add one"));
        app.users = UsersState::Loaded(vec![
            User {
                name: "james".into(),
                admin: false,
            },
            User {
                name: "olive".into(),
                admin: false,
            },
        ]);
        let text = render(&app);
        assert!(text.contains("Logins:") && text.contains("james") && text.contains("olive"));
        assert!(text.contains("n new") && text.contains("d delete"));
    }

    #[test]
    fn user_prompts_are_labelled_and_passwords_masked() {
        let mut app = app();
        app.window = MODULES.iter().find(|m| m.id == "nt4-domain");
        app.mode = Mode::UserName;
        assert!(render(&app).contains("New user name"));
        app.mode = Mode::UserPassword;
        app.input = "hunter2".into();
        let text = render(&app);
        assert!(text.contains("Password for ?") && text.contains("> *******"));
        assert!(!text.contains("hunter2"));
        app.mode = Mode::UserRepeat;
        assert!(render(&app).contains("Repeat the password"));
        app.mode = Mode::UserDelete;
        let text = render(&app);
        assert!(text.contains("Delete ?") && text.contains("y delete"));
    }
}
