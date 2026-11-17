//! the My Server window: what the signed-in server is (General) and the one dangerous thing you
//! can do to it (Advanced): take lanlord back off.

use lanlord_model::server::ServerFacts;

use crate::log::{emit, Kind};

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
    host: String,
    preview: Remote<Vec<String>>,
    running: Remote<Vec<String>>,
    remove_admins: bool,
    /// what you typed to confirm; it must equal the host name.
    typed: String,
}

impl PurgeBox {
    pub fn host(&self) -> &str {
        &self.host
    }

    /// what a purge would remove, once the preview has come back.
    pub fn preview(&self) -> Option<&[String]> {
        self.preview.data.as_deref()
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    pub fn remove_admins(&self) -> bool {
        self.remove_admins
    }

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

    /// list what a purge would remove with the current choices; nothing is changed. false when
    /// the host was busy and nothing was started.
    fn look(&mut self, services: &Services) -> bool {
        let (host, admins) = (self.host.clone(), self.remove_admins);
        let job = services.spawn(&self.host, move |gateway, ask| {
            gateway.purge_preview(&host, admins, ask)
        });
        if job.is_none() {
            return false;
        }
        self.preview = Remote::default();
        self.preview.track(
            &self.host,
            job,
            "checking what would be removed...",
            "this is everything that would be removed",
            true,
        );
        true
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
    tab: usize,
    facts: Remote<ServerFacts>,
    purge: Option<PurgeBox>,
    /// a purge finished on this host; the desktop logs off and forgets it.
    purged: Option<String>,
}

impl MyServer {
    pub fn tab(&self) -> usize {
        self.tab
    }

    /// the server's facts and the job reading them.
    pub fn facts(&self) -> &Remote<ServerFacts> {
        &self.facts
    }

    /// the purge box, while it is open.
    pub fn purge(&self) -> Option<&PurgeBox> {
        self.purge.as_ref()
    }

    /// can a purge be asked for? lanlord is on the server, nothing runs against it and no box
    /// is open already.
    fn can_purge(&self, idle: bool) -> bool {
        let installed = self.facts.data.as_ref().map(|f| f.lanlord_installed);
        installed == Some(true) && idle && self.purge.is_none()
    }

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
        let ready = self.can_purge(idle);
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
                edit: Msg::Typed,
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
        })
    }

    /// act on a message. the rules the window shows are enforced here too, whatever the front
    /// end sends: no purge without the typed host name, no cancelling a purge that runs.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        match msg {
            Msg::Tab(tab) => self.tab = tab,
            Msg::AskPurge => {
                if self.can_purge(!services.busy(host)) {
                    self.ask_to_purge(services, host);
                }
            }
            Msg::Cancel => {
                if !self.purge.as_ref().is_some_and(|p| p.running.busy()) {
                    self.purge = None;
                }
            }
            Msg::ToggleAdmins => {
                let is_root = self.facts.data.as_ref().is_some_and(|f| f.is_root);
                if let Some(purge) = self.purge.as_mut() {
                    if is_root
                        && !purge.running.busy()
                        && !purge.preview.busy()
                        && !services.busy(&purge.host)
                    {
                        purge.remove_admins = !purge.remove_admins;
                        // keep the new choice only when its preview started
                        if !purge.look(services) {
                            purge.remove_admins = !purge.remove_admins;
                        }
                    }
                }
            }
            Msg::Typed(text) => {
                if let Some(purge) = self.purge.as_mut() {
                    purge.typed = text;
                }
            }
            Msg::Remove => {
                if let Some(purge) = self.purge.as_mut() {
                    if purge.confirmed(services) {
                        purge.remove(services);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use std::time::{Duration, Instant};

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    fn until(window: &mut MyServer, done: impl Fn(&MyServer) -> bool) {
        let start = Instant::now();
        while !done(window) {
            window.refresh_purge();
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// a window whose purge box is open with its preview back.
    fn asked(fake: &Arc<Fake>, services: &Services) -> MyServer {
        let mut window = MyServer::default();
        window.refresh(services, "me@gw");
        let start = Instant::now();
        while !window.facts.belongs_to("me@gw") {
            window.refresh(services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        window.update(Msg::AskPurge, services, "me@gw");
        until(&mut window, |w| {
            w.purge().is_some_and(|p| p.preview().is_some())
        });
        assert!(!services.busy("me@gw"));
        assert_eq!(fake.calls().len(), 2, "the facts and the preview");
        window
    }

    #[test]
    fn a_wrong_host_name_purges_nothing() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gx".into()), &services, "me@gw");
        window.update(Msg::Remove, &services, "me@gw");
        assert!(!services.busy("me@gw"));
        assert!(!fake.calls().iter().any(|c| c.starts_with("purge me@gw")));
    }

    #[test]
    fn the_admins_toggle_waits_for_the_preview() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::ToggleAdmins, &services, "me@gw");
        assert!(window.purge().is_some_and(|p| p.remove_admins()));
        // the new preview is still running, so a second toggle is ignored
        window.update(Msg::ToggleAdmins, &services, "me@gw");
        assert!(window.purge().is_some_and(|p| p.remove_admins()));
    }

    #[test]
    fn the_admins_toggle_is_ignored_while_the_host_is_busy() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        *fake.password_prompt.lock().unwrap() = Some("pw".into());
        assert!(services.apply("me@gw", Vec::new()));
        window.update(Msg::ToggleAdmins, &services, "me@gw");
        let purge = window.purge().unwrap();
        assert!(!purge.remove_admins(), "the choice did not change");
        assert!(purge.preview().is_some(), "the preview stays valid");
        // let the waiting task finish
        let request = loop {
            if let Some(request) = services.next_request() {
                break request;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        request.reply.send(None).unwrap();
    }

    #[test]
    fn a_running_purge_cannot_be_cancelled_and_still_reports() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gw".into()), &services, "me@gw");
        window.update(Msg::Remove, &services, "me@gw");
        window.update(Msg::Cancel, &services, "me@gw");
        assert!(window.purging(), "the box stays while it runs");
        until(&mut window, |w| !w.purging());
        assert_eq!(window.take_purged().as_deref(), Some("me@gw"));
        assert!(fake
            .calls()
            .contains(&"purge me@gw admins=false".to_string()));
    }

    #[test]
    fn the_tabs_switch_pages_and_the_purge_box_follows_its_messages() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let tab_of = |window: &MyServer| match &window.view(true)[0] {
            Node::Tabs { current, .. } => *current,
            _ => panic!("the tabs come first"),
        };
        let mut window = MyServer::default();
        assert_eq!(tab_of(&window), GENERAL);
        // without the facts there is nothing to purge yet
        window.update(Msg::AskPurge, &services, "me@gw");
        assert!(!window.purging());
        window.update(Msg::Tab(1), &services, "me@gw");
        assert_eq!(tab_of(&window), 1);
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gw".into()), &services, "me@gw");
        assert_eq!(window.purge().unwrap().typed(), "me@gw");
        let dialog = window.dialog(&services).expect("the box is open");
        assert_eq!(dialog.title, "Remove LANlord");
        window.update(Msg::Cancel, &services, "me@gw");
        assert!(!window.purging());
    }
}
