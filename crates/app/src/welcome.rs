//! the Welcome window, in the style of the nt 4 and 95 welcome screen: title art, a button for
//! each topic and a text panel that explains the chosen one. it shows after logon while the
//! account keeps its tick box on, and the choice is remembered per admin account in the
//! client's preferences.

use std::path::PathBuf;

use crate::desktop::Target;
use crate::view::{Button, Node};

/// the box at the bottom of the window.
pub const SHOW_EACH_TIME: &str = "Show this Welcome screen each time you log on";

/// what a click in the window means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    /// show another topic.
    Topic(usize),
    /// open the window the topic is about.
    Open,
    /// open My Server on its Compatibility tab.
    OpenCompat,
    Close,
    /// tick or clear the show-each-time box.
    Toggle,
}

/// the width of the topic buttons: the longest label and some padding.
const TOPIC_BUTTON: f32 = 100.0;
/// the width of the text panel.
pub const PANEL: f32 = 400.0;
/// how many characters a line of the panel holds.
const LINE: usize = 60;

/// the paragraphs of a topic broken into lines of at most `LINE` characters, with a blank line
/// between paragraphs.
fn lines(text: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (n, paragraph) in text.iter().enumerate() {
        if n > 0 {
            out.push(String::new());
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + word.len() > LINE {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

struct Topic {
    button: &'static str,
    title: &'static str,
    text: &'static [&'static str],
    open: &'static str,
    /// the window the topic opens: the id of a desktop icon, or "components".
    target: &'static str,
}

const TOPICS: &[Topic] = &[
    Topic {
        button: "Networks",
        title: "Networks",
        text: &[
            "A LANlord network is one that LANlord serves with DHCP, DNS and the domain.",
            "New networks get a random private /24, for example 192.0.2.0/24, with DHCP on and no default gateway. Its computers have no internet unless you turn it on for the whole network or for chosen computers.",
        ],
        open: "Open Networks",
        target: "networks",
    },
    Topic {
        button: "People",
        title: "People",
        text: &[
            "People are the accounts that sign in to the domain, one for each person.",
            "Add them, set their passwords and put them in groups in the NT4 Domain window.",
        ],
        open: "Open NT4 Domain",
        target: "nt4-domain",
    },
    Topic {
        button: "My Server",
        title: "My Server",
        text: &[
            "My Server is the computer LANlord runs on: its name, its address and what it is doing.",
            "Its Compatibility tab checks what the server can do for the components, such as running docker in a container, and shows how to fix what is missing.",
            "Its Advanced page holds the actions that cannot be undone, such as a purge.",
        ],
        open: "Open My Server",
        target: "my-server",
    },
    Topic {
        button: "Components",
        title: "Components",
        text: &[
            "Components are the services LANlord can run for your networks, such as the gateway, DNS and the NT4 domain.",
            "The setup wizard switches them on and picks the networks each one joins. Only the ones switched on get a desktop icon.",
        ],
        open: "Open Components",
        target: "components",
    },
];

pub struct Welcome {
    topic: usize,
    show: bool,
    /// the account and the preferences file are only used where there are preferences.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    user: String,
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    prefs: Option<PathBuf>,
    done: bool,
    open: Option<Target>,
    compat: bool,
}

impl Welcome {
    /// `prefs` is the client's preferences file, where the tick box is remembered.
    pub fn new(prefs: Option<PathBuf>) -> Self {
        Self {
            topic: 0,
            show: true,
            user: String::new(),
            prefs,
            done: false,
            open: None,
            compat: false,
        }
    }

    /// `account` signed in: read its choice, and say whether the window should open now.
    pub fn logon(&mut self, account: &str) -> bool {
        self.user = account.to_string();
        self.show = self.stored(account);
        self.show
    }

    /// the window is opened by hand (from the menu): start on the first topic.
    pub fn reopen(&mut self) {
        self.topic = 0;
        self.done = false;
    }

    fn stored(&self, account: &str) -> bool {
        #[cfg(feature = "native")]
        if let Some(path) = &self.prefs {
            return lanlord_core::prefs::load(path).welcome(account);
        }
        let _ = account;
        true
    }

    fn store(&self) {
        #[cfg(feature = "native")]
        if let Some(path) = &self.prefs {
            let mut prefs = lanlord_core::prefs::load(path);
            prefs.set_welcome(&self.user, self.show);
            let _ = lanlord_core::prefs::save(path, &prefs);
        }
    }

    pub fn shows_at_logon(&self) -> bool {
        self.show
    }

    /// the window asked to close.
    pub fn take_done(&mut self) -> bool {
        std::mem::take(&mut self.done)
    }

    /// the window a topic's button asked to open.
    pub fn take_open(&mut self) -> Option<Target> {
        self.open.take()
    }

    /// the Compatibility button was clicked, once: the desktop opens My Server on that tab.
    pub fn take_compat(&mut self) -> bool {
        std::mem::take(&mut self.compat)
    }

    fn target(&self, id: &str, icons: &[Target]) -> Option<Target> {
        if id == "components" {
            return Some(Target::Components);
        }
        icons.iter().copied().find(|t| t.id() == id)
    }

    /// the window's tree; `icons` are the windows the desktop has, so a topic only opens one
    /// that exists.
    pub fn view(&self, icons: &[Target]) -> Vec<Node<Msg>> {
        let topic = &TOPICS[self.topic];
        // the chosen topic is drawn pressed (a front end without that look greys it)
        let buttons = TOPICS
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let chosen = i == self.topic;
                Node::Button(
                    Button::new(t.button, !chosen, Msg::Topic(i))
                        .selected(chosen)
                        .width(TOPIC_BUTTON),
                )
            })
            .collect();
        // every topic takes the same number of lines, so the window does not change size
        let rows = TOPICS
            .iter()
            .map(|t| lines(t.text).len())
            .max()
            .unwrap_or(0);
        let mut text: Vec<Node<Msg>> = lines(topic.text)
            .into_iter()
            .chain(std::iter::repeat(String::new()))
            .take(rows)
            .map(Node::Text)
            .collect();
        text.push(Node::Gap(8.0));
        let mut buttons_row = vec![Node::Button(Button::new(
            topic.open,
            self.target(topic.target, icons).is_some(),
            Msg::Open,
        ))];
        if topic.target == "my-server" {
            buttons_row.push(Node::Gap(6.0));
            buttons_row.push(Node::Button(Button::new(
                "Open Compatibility",
                self.target(topic.target, icons).is_some(),
                Msg::OpenCompat,
            )));
        }
        text.push(Node::Row(buttons_row));
        vec![
            Node::Header {
                icon: "my-server",
                title: "Welcome to LANlord".into(),
                lines: vec!["Retro networks, run from one desktop.".into()],
            },
            Node::Gap(8.0),
            Node::Columns(vec![
                (
                    Some(PANEL),
                    vec![Node::Group {
                        title: topic.title.into(),
                        body: text,
                    }],
                ),
                (Some(TOPIC_BUTTON), buttons),
            ]),
            Node::Gap(8.0),
            Node::Check {
                label: SHOW_EACH_TIME.into(),
                checked: self.show,
                enabled: true,
                toggle: Msg::Toggle,
            },
            Node::Separator,
            Node::Buttons(vec![Button::new("Close", true, Msg::Close)]),
        ]
    }

    pub fn update(&mut self, msg: Msg, icons: &[Target]) {
        match msg {
            Msg::Topic(i) if i < TOPICS.len() => self.topic = i,
            Msg::Topic(_) => {}
            Msg::Open => self.open = self.target(TOPICS[self.topic].target, icons),
            Msg::OpenCompat => {
                self.open = self.target("my-server", icons);
                self.compat = self.open.is_some();
            }
            Msg::Close => self.done = true,
            Msg::Toggle => {
                self.show = !self.show;
                self.store();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_takes_the_same_number_of_lines() {
        let rows = |w: &mut Welcome| match &w.view(&[])[2] {
            Node::Columns(c) => match &c[0].1[0] {
                Node::Group { body, .. } => body.len(),
                _ => panic!("no group"),
            },
            _ => panic!("no columns"),
        };
        let mut welcome = Welcome::new(None);
        let first = rows(&mut welcome);
        for topic in 1..TOPICS.len() {
            welcome.update(Msg::Topic(topic), &[]);
            assert_eq!(rows(&mut welcome), first);
        }
        assert!(TOPICS
            .iter()
            .all(|t| lines(t.text).iter().all(|l| l.len() <= LINE)));
    }

    fn texts(nodes: &[Node<Msg>]) -> String {
        let mut out = String::new();
        for node in nodes {
            match node {
                Node::Text(t) => out += t,
                Node::Group { title, body } => {
                    out += title;
                    out += &texts(body);
                }
                Node::Button(b) => out += &b.label,
                Node::Row(items) => out += &texts(items),
                Node::Columns(cols) => cols.iter().for_each(|(_, c)| out += &texts(c)),
                Node::Check { label, .. } => out += label,
                Node::Buttons(b) => b.iter().for_each(|b| out += &b.label),
                _ => {}
            }
            out.push('\n');
        }
        out
    }

    fn icons() -> Vec<Target> {
        crate::desktop::icons(None)
    }

    #[test]
    fn the_tree_has_the_topics_networks_first_and_the_checkbox() {
        let w = Welcome::new(None);
        let text = texts(&w.view(&icons()));
        let (net, people, server, comps) = (
            text.find("Networks\n").unwrap(),
            text.find("People\n").unwrap(),
            text.find("My Server\n").unwrap(),
            text.find("Components\n").unwrap(),
        );
        assert!(net < people && people < server && server < comps);
        assert!(text.contains("Open Networks"));
        assert!(text.contains("no internet unless"));
        assert!(text.contains(SHOW_EACH_TIME));
        assert!(text.contains("Close"));
        assert!(w
            .view(&icons())
            .iter()
            .any(|n| matches!(n, Node::Check { checked: true, .. })));
    }

    #[test]
    fn the_my_server_topic_mentions_and_opens_the_compatibility_tab() {
        let mut w = Welcome::new(None);
        w.update(Msg::Topic(2), &icons());
        let text = texts(&w.view(&icons()));
        assert!(text.contains("Compatibility tab"));
        assert!(text.contains("Open Compatibility"));
        w.update(Msg::OpenCompat, &icons());
        assert_eq!(w.take_open(), Some(Target::MyServer));
        assert!(w.take_compat());
        assert!(!w.take_compat(), "once");
        w.update(Msg::Topic(0), &icons());
        assert!(!texts(&w.view(&icons())).contains("Open Compatibility"));
    }

    #[test]
    fn a_topic_button_opens_its_window_only_when_it_exists() {
        let mut w = Welcome::new(None);
        w.update(Msg::Open, &icons());
        assert_eq!(w.take_open(), Some(Target::Networks));
        w.update(Msg::Topic(1), &icons());
        w.update(Msg::Open, &icons());
        assert_eq!(w.take_open(), None, "nt4 domain is not switched on");
        w.update(Msg::Topic(3), &icons());
        w.update(Msg::Open, &icons());
        assert_eq!(w.take_open(), Some(Target::Components));
        w.update(Msg::Close, &icons());
        assert!(w.take_done());
    }

    #[cfg(feature = "native")]
    #[test]
    fn the_checkbox_is_remembered_per_account() {
        let dir = std::env::temp_dir().join(format!("lanlord-welcome-app-{}", std::process::id()));
        let file = dir.join("prefs");
        let mut w = Welcome::new(Some(file.clone()));
        assert!(w.logon("dana"), "shown on the first logon");
        w.update(Msg::Toggle, &icons());
        assert!(!w.shows_at_logon());
        let mut again = Welcome::new(Some(file.clone()));
        assert!(!again.logon("dana"));
        assert!(again.logon("lee"), "another account still gets it");
        again.logon("dana");
        again.update(Msg::Toggle, &icons());
        assert!(Welcome::new(Some(file)).logon("dana"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
