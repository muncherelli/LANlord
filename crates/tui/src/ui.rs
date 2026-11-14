//! drawing: a classic gray-on-teal desktop with navy title bars.

use lanlord_core::log::Kind;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, Mode};

const DESKTOP: Color = Color::Rgb(0, 128, 128);
const NAVY: Color = Color::Rgb(0, 0, 128);
const GRAY: Color = Color::Rgb(192, 192, 192);

fn window(title: String) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Double)
        .title(title)
        .style(Style::new().bg(GRAY).fg(Color::Black))
}

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

    let items: Vec<ListItem> = if app.hosts.is_empty() {
        vec![ListItem::new("(none - press a)")]
    } else {
        app.hosts
            .iter()
            .map(|h| ListItem::new(h.as_str()))
            .collect()
    };
    let list = List::new(items)
        .block(window(" Hosts ".into()))
        .highlight_style(Style::new().bg(NAVY).fg(Color::White));
    let mut state = ListState::default();
    if !app.hosts.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, body[0], &mut state);

    let height = body[1].height.saturating_sub(2) as usize;
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
    frame.render_widget(console, body[1]);

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
    let shown = if app.mode == Mode::Password {
        "*".repeat(app.input.chars().count())
    } else {
        app.input.clone()
    };
    let input = Paragraph::new(format!("{prompt}{shown}")).block(window(label));
    frame.render_widget(input, rows[2]);
    if app.mode != Mode::Normal {
        let x = rows[2].x + 1 + (prompt.len() + app.input.chars().count()) as u16;
        frame.set_cursor_position((x.min(rows[2].right().saturating_sub(2)), rows[2].y + 1));
    }

    let hint = match app.mode {
        Mode::Normal => " up/down select   enter command   b bootstrap   a add host   q quit ",
        Mode::Password => " enter send   esc cancel ",
        Mode::Command => " enter run   esc back ",
        Mode::AddHost => " enter save   esc cancel ",
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::new().bg(GRAY).fg(Color::Black)),
        rows[3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_core::log::Line as LogLine;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::sync::mpsc;

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
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
}
