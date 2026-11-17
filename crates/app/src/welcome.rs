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
    Close,
    /// tick or clear the show-each-time box.
    Toggle,
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
        let buttons = TOPICS
            .iter()
            .enumerate()
            .map(|(i, t)| Node::Button(Button::new(t.button, i != self.topic, Msg::Topic(i))))
            .collect();
        let mut text: Vec<Node<Msg>> = topic.text.iter().map(|l| Node::Text((*l).into())).collect();
        text.push(Node::Gap(8.0));
        text.push(Node::Button(Button::new(
            topic.open,
            self.target(topic.target, icons).is_some(),
            Msg::Open,
        )));
        vec![
            Node::Header {
                icon: "my-server",
                title: "Welcome to LANlord".into(),
                lines: vec!["Retro networks, run from one desktop.".into()],
            },
            Node::Gap(8.0),
            Node::Columns(vec![
                (
                    Some(400.0),
                    vec![Node::Group {
                        title: topic.title.into(),
                        body: text,
                    }],
                ),
                (Some(120.0), buttons),
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
