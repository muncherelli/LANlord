//! the My Server window: what the signed-in server is (General) and the one dangerous thing you
//! can do to it (Advanced): take lanlord back off.

use lanlord_core::log::{emit, Kind};
use lanlord_core::server::ServerFacts;

use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node};

pub const GENERAL: usize = 0;

/// what a click or a keystroke in the window means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Tab(usize),
    AskPurge,
    /// the "also remove the administrator accounts" tick.
    ToggleAdmins,
    /// the confirmation field now holds this text.
    Typed(String),
    Cancel,
    Remove,
}

/// the confirmation box for a purge: what would go, who may type the name, the running job.
pub struct PurgeBox {
    pub host: String,
    pub preview: Remote<Vec<String>>,
    pub running: Remote<Vec<String>>,
    pub remove_admins: bool,
    /// what you typed to confirm; it must equal the host name.
    pub typed: String,
}

impl PurgeBox {
    fn new(host: &str, services: &Services) -> Self {
        let mut purge = Self {
            host: host.to_string(),
            preview: Remote::default(),
            running: Remote::default(),
            remove_admins: false,
            typed: String::new(),
        };
        purge.look(services);
        purge
    }

    /// list what a purge would remove with the current choices; nothing is changed.
    fn look(&mut self, services: &Services) {
        let (host, admins) = (self.host.clone(), self.remove_admins);
        let job = services.spawn(&self.host, move |gateway, ask| {
            gateway.purge_preview(&host, admins, ask)
        });
        self.preview = Remote::default();
        self.preview.track(
            &self.host,
            job,
            "checking what would be removed...",
            "this is everything that would be removed",
            true,
        );
    }

    fn confirmed(&self, services: &Services) -> bool {
        self.typed == self.host
            && self.preview.data.is_some()
            && !self.preview.busy()
            && !self.running.busy()
            && !services.busy(&self.host)
    }

    fn remove(&mut self, services: &Services) {
        let (host, admins) = (self.host.clone(), self.remove_admins);
        let job = services.spawn(&self.host, move |gateway, ask| {
            gateway.purge(&host, admins, ask)
        });
        self.running.track(
            &self.host,
            job,
            "removing lanlord...",
            "lanlord was removed",
            false,
        );
    }
}

#[derive(Default)]
pub struct MyServer {
    pub tab: usize,
    pub facts: Remote<ServerFacts>,
    pub purge: Option<PurgeBox>,
    /// a purge finished on this host; the desktop logs off and forgets it.
    purged: Option<String>,
}

impl MyServer {
    /// the host whose purge just finished, once.
    pub fn take_purged(&mut self) -> Option<String> {
        self.purged.take()
    }

    /// is the purge box open (or running)?
    pub fn purging(&self) -> bool {
        self.purge.is_some()
    }

    /// open the box that asks before a purge.
    pub fn ask_to_purge(&mut self, services: &Services, host: &str) {
        if self.purge.is_none() {
            self.purge = Some(PurgeBox::new(host, services));
        }
    }

    /// pick up finished reads and start one when the window shows another server.
    pub fn refresh(&mut self, services: &Services, host: &str) {
        self.facts.poll();
        if self.facts.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| gateway.server_facts(&target, ask));
            self.facts
                .track(host, job, "reading the server...", "up to date", true);
        }
    }

    /// pick up the purge box's jobs; a finished purge closes it and is reported once.
    pub fn refresh_purge(&mut self) {
        let Some(purge) = self.purge.as_mut() else {
            return;
        };
        purge.preview.poll();
        if purge.running.poll() {
            for line in purge.running.data.take().unwrap_or_default() {
                emit(Kind::Info, line);
            }
            emit(
                Kind::Info,
                format!("lanlord was removed from {}", purge.host),
            );
            self.purged = self.purge.take().map(|p| p.host);
        }
    }

    /// the window's contents; `idle` when nothing runs against the server.
    pub fn view(&self, idle: bool) -> Vec<Node<Msg>> {
        let page = if self.tab == GENERAL {
            self.general()
        } else {
            self.advanced(idle)
        };
        vec![
            Node::Tabs {
                titles: vec!["General", "Advanced"],
                current: self.tab,
                pick: Msg::Tab,
                body: page,
            },
            Node::Gap(6.0),
            view::status(&self.facts),
        ]
    }

    fn general(&self) -> Vec<Node<Msg>> {
        let Some(facts) = &self.facts.data else {
            return vec![Node::Text("Reading the server...".into())];
        };
        let lanlord = match (&facts.lanlord_installed, &facts.lanlord_version) {
            (false, _) => "Not installed".to_string(),
            (true, Some(version)) => format!("Installed, version {version}"),
            (true, None) => "Installed".to_string(),
        };
        let mut computer: Vec<Node<Msg>> = [
            ("Server", facts.hostname.clone()),
            ("Signed in as", facts.user.clone()),
            ("System", facts.os.clone()),
            ("Kernel", facts.kernel.clone()),
            ("Up for", facts.uptime()),
            ("Processors", facts.cpus.to_string()),
            (
                "Memory",
                format!(
                    "{} MB, {} MB free",
                    facts.mem_total_mb, facts.mem_available_mb
                ),
            ),
            ("LANlord", lanlord),
        ]
        .into_iter()
        .map(|(label, value)| Node::field(label, value))
        .collect();
        if let Some(applied) = &facts.last_applied {
            computer.push(Node::field("Last applied", applied.clone()));
        }
        let interfaces = facts
            .interfaces
            .iter()
            .filter(|i| !i.loopback())
            .map(|iface| {
                let address = iface.addresses.first().map_or("no address", String::as_str);
                Node::Text(format!(
                    "{}   {}   MTU {}   {}",
                    iface.name,
                    address,
                    iface.mtu,
                    iface.link()
                ))
            })
            .collect();
        // the environment check (container, nesting, keyctl) joins these once core::env lands
        vec![
            Node::Group {
                title: "Computer".into(),
                body: computer,
            },
            Node::Gap(4.0),
            Node::Group {
                title: "Network interfaces".into(),
                body: interfaces,
            },
        ]
    }

    fn advanced(&self, idle: bool) -> Vec<Node<Msg>> {
        let installed = self.facts.data.as_ref().map(|f| f.lanlord_installed);
        let ready = installed == Some(true) && idle && self.purge.is_none();
        let mut body = vec![
            Node::Gap(4.0),
            Node::Text(
                "Removes the containers, files, service account and sudo rules that \
                 LANlord put on this server. Your own login stays."
                    .into(),
            ),
            Node::Gap(6.0),
            Node::Button(Button::new(
                "Remove LANlord from this server...",
                ready,
                Msg::AskPurge,
            )),
        ];
        if installed == Some(false) {
            body.push(Node::Gap(4.0));
            body.push(Node::Text(
                "LANlord is not installed on this server.".into(),
            ));
        }
        vec![Node::Danger(vec![Node::Group {
            title: "Danger zone".into(),
            body,
        }])]
    }

    /// the confirmation box, while a purge is being asked for or run.
    pub fn dialog(&self, services: &Services) -> Option<Dialog<Msg>> {
        let purge = self.purge.as_ref()?;
        let is_root = self.facts.data.as_ref().is_some_and(|f| f.is_root);
        let working = purge.running.busy();
        let mut body = vec![
            Node::Heading(format!("Remove LANlord from {}?", purge.host)),
            Node::Text("This cannot be undone. It would remove:".into()),
            Node::Gap(4.0),
            Node::Lines {
                id: "purge-lines",
                lines: purge.preview.data.clone().unwrap_or_default(),
                height: 150.0,
            },
            Node::Gap(4.0),
            Node::Check {
                label: "Also remove the administrator accounts".into(),
                checked: purge.remove_admins,
                enabled: is_root && !working && !purge.preview.busy(),
                toggle: Msg::ToggleAdmins,
            },
        ];
        if !is_root {
            body.push(Node::Text(
                "(Removing the accounts needs a root login.)".into(),
            ));
        }
        body.extend([
            Node::Gap(4.0),
            Node::Text(format!("Type {} to confirm:", purge.host)),
            Node::Input {
                value: purge.typed.clone(),
                width: 240.0,
                masked: false,
                focus: false,
                edit: Msg::Typed,
                enter: None,
            },
            Node::Gap(6.0),
            Node::Buttons(vec![
                Button::new("Cancel", !working, Msg::Cancel),
                Button::new("Remove", purge.confirmed(services), Msg::Remove),
            ]),
        ]);
        let note = purge
            .running
            .note
            .clone()
            .or_else(|| purge.preview.note.clone());
        if let Some((error, text)) = note {
            body.push(Node::Status { text, error });
        }
        Some(Dialog {
            id: "purge-box",
            title: "Remove LANlord".into(),
            body,
            escape: None,
        })
    }

    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        match msg {
            Msg::Tab(tab) => self.tab = tab,
            Msg::AskPurge => self.ask_to_purge(services, host),
            Msg::Cancel => self.purge = None,
            Msg::ToggleAdmins => {
                if let Some(purge) = self.purge.as_mut() {
                    purge.remove_admins = !purge.remove_admins;
                    purge.look(services);
                }
            }
            Msg::Typed(text) => {
                if let Some(purge) = self.purge.as_mut() {
                    purge.typed = text;
                }
            }
            Msg::Remove => {
                if let Some(purge) = self.purge.as_mut() {
                    purge.remove(services);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    #[test]
    fn the_tabs_switch_pages_and_the_purge_box_follows_its_messages() {
        let services = Services::new(Arc::new(Fake::new()), Wake::none());
        let mut window = MyServer::default();
        let tab_of = |window: &MyServer| match &window.view(true)[0] {
            Node::Tabs { current, .. } => *current,
            _ => panic!("the tabs come first"),
        };
        assert_eq!(tab_of(&window), GENERAL);
        window.update(Msg::Tab(1), &services, "me@gw");
        assert_eq!(tab_of(&window), 1);
        assert!(window.dialog(&services).is_none());
        window.update(Msg::AskPurge, &services, "me@gw");
        assert!(window.purging());
        window.update(Msg::Typed("me@gw".into()), &services, "me@gw");
        assert_eq!(window.purge.as_ref().unwrap().typed, "me@gw");
        let dialog = window.dialog(&services).expect("the box is open");
        assert_eq!(dialog.title, "Remove LANlord");
        window.update(Msg::Cancel, &services, "me@gw");
        assert!(!window.purging());
    }
}
