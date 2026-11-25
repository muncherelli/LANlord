//! the hosting window: a console-style tree on the left (services, servers, the gateway,
//! profiles) and the details of the selected node on the right. it is read-only and shows what
//! the gateway already keeps: the machines and the facts of their last detection. nothing is
//! detected or changed from here; the debug window has the buttons for that.

use std::collections::{HashMap, HashSet};

use lanlord_model::hosting::{HostingFacts, SiteFact, SiteKind};

use crate::services::Services;
use crate::tasks::Job;
use crate::view::{Dialog, Node, Table, Tree, TreeRow};

/// how wide the tree is, in layout units.
const TREE_WIDTH: f32 = 190.0;

/// the profiles of the web technologies, each with what it will be.
const PROFILES: [(&str, &str); 3] = [
    ("Static HTML", "Plain files served as they are."),
    ("ColdFusion", "Pages run by a ColdFusion engine."),
    ("PHP", "Pages run by PHP."),
];

/// the empty state of the Services node.
const NO_SERVICES: &str =
    "No services yet. Services you add here are set up on every backend at once.";

/// a node of the tree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
enum Id {
    #[default]
    Root,
    Services,
    Servers,
    Server(String),
    /// the sites of one kind of a server: web sites, ftp sites or smtp.
    Folder(String, SiteKind),
    Site(String, SiteKind, u32),
    Gateway,
    Http,
    Forwards,
    Profiles,
    Profile(usize),
}

/// what a click or a key means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    /// the row `0` of the tree was picked.
    Select(usize),
    /// the branch at row `0` was opened or closed.
    Open(usize, bool),
    /// the row `0` of the details list was picked: go to that child of the selected node.
    Child(usize),
    /// a row of a list that goes nowhere.
    Nothing,
}

/// what the gateway kept for a machine, or why it could not be read.
enum Kept {
    Facts(Option<Box<HostingFacts>>),
    Failed(String),
}

type Read = Vec<(String, Result<Option<Box<HostingFacts>>, String>)>;

/// facts being read from the gateway for these machines.
struct Fetch {
    job: Job<Read>,
    host: String,
    machines: Vec<String>,
}

pub struct HostingTree {
    remote: crate::remote::Remote<Vec<String>>,
    /// the host everything below belongs to.
    shown_host: Option<String>,
    open: HashSet<Id>,
    selected: Id,
    can_read_facts: bool,
    facts: HashMap<String, Kept>,
    fetching: Option<Fetch>,
}

impl Default for HostingTree {
    fn default() -> Self {
        Self {
            remote: crate::remote::Remote::default(),
            shown_host: None,
            open: HashSet::from([Id::Root, Id::Servers]),
            selected: Id::Root,
            can_read_facts: false,
            facts: HashMap::new(),
            fetching: None,
        }
    }
}

/// "2026-01-01 12:00 UTC" for seconds since the epoch, or "never" for 0.
fn when(secs: u64) -> String {
    i64::try_from(secs)
        .ok()
        .filter(|s| *s > 0)
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map_or_else(
            || "never".to_string(),
            |t| t.format("%Y-%m-%d %H:%M UTC").to_string(),
        )
}

/// the name of a folder of sites.
fn folder_name(kind: SiteKind) -> &'static str {
    match kind {
        SiteKind::Web => "Web Sites",
        SiteKind::Ftp => "FTP Sites",
        SiteKind::Smtp => "SMTP",
    }
}

fn site_name(site: &SiteFact) -> String {
    if site.comment.is_empty() {
        format!("Site {}", site.id)
    } else {
        site.comment.clone()
    }
}

fn bindings(site: &SiteFact) -> Vec<String> {
    site.bindings
        .iter()
        .map(|b| {
            let ip = if b.ip.is_empty() { "*" } else { &b.ip };
            format!("{ip}:{}:{}", b.port, b.host)
        })
        .collect()
}

fn list(
    id: &'static str,
    headings: Vec<&'static str>,
    widths: Vec<f32>,
    rows: Vec<Vec<String>>,
) -> Node<Msg> {
    Node::Table(Table {
        id,
        headings,
        widths,
        rows,
        selected: None,
        bad: Vec::new(),
        height: 150.0,
        steady: true,
        select: Msg::Child,
    })
}

/// a list whose rows are not nodes.
fn plain(
    id: &'static str,
    headings: Vec<&'static str>,
    widths: Vec<f32>,
    rows: Vec<Vec<String>>,
) -> Node<Msg> {
    let Node::Table(table) = list(id, headings, widths, rows) else {
        unreachable!("list builds a table")
    };
    Node::Table(Table {
        select: |_| Msg::Nothing,
        ..table
    })
}

impl HostingTree {
    /// may the person read the facts kept for a machine? set from their grants; the server
    /// decides anyway.
    pub fn set_can_read_facts(&mut self, can: bool) {
        self.can_read_facts = can;
    }

    fn names(&self) -> &[String] {
        self.remote.data.as_deref().unwrap_or(&[])
    }

    fn facts_of(&self, machine: &str) -> Option<&HostingFacts> {
        match self.facts.get(machine) {
            Some(Kept::Facts(Some(facts))) => Some(facts),
            _ => None,
        }
    }

    fn sites(&self, machine: &str, kind: SiteKind) -> Vec<&SiteFact> {
        self.facts_of(machine)
            .map(|f| f.sites.iter().filter(|s| s.kind == kind).collect())
            .unwrap_or_default()
    }

    fn children(&self, id: &Id) -> Vec<Id> {
        match id {
            Id::Root => vec![Id::Services, Id::Servers, Id::Gateway, Id::Profiles],
            Id::Servers => self.names().iter().cloned().map(Id::Server).collect(),
            Id::Server(m) => [SiteKind::Web, SiteKind::Ftp, SiteKind::Smtp]
                .into_iter()
                .map(|k| Id::Folder(m.clone(), k))
                .collect(),
            Id::Folder(m, kind) => self
                .sites(m, *kind)
                .iter()
                .map(|s| Id::Site(m.clone(), *kind, s.id))
                .collect(),
            Id::Gateway => vec![Id::Http, Id::Forwards],
            Id::Profiles => (0..PROFILES.len()).map(Id::Profile).collect(),
            _ => Vec::new(),
        }
    }

    fn label(&self, id: &Id) -> String {
        match id {
            Id::Root => "LANlord Hosting".into(),
            Id::Services => "Services".into(),
            Id::Servers => "Servers".into(),
            Id::Server(m) if self.facts_of(m).is_some_and(HostingFacts::iis_seen) => {
                format!("{m}  (IIS 4)")
            }
            Id::Server(m) => m.clone(),
            Id::Folder(_, kind) => folder_name(*kind).into(),
            Id::Site(m, kind, n) => self
                .sites(m, *kind)
                .iter()
                .find(|s| s.id == *n)
                .map_or_else(|| format!("Site {n}"), |s| site_name(s)),
            Id::Gateway => "Gateway".into(),
            Id::Http => "HTTP (Caddy)".into(),
            Id::Forwards => "Port forwards".into(),
            Id::Profiles => "Profiles".into(),
            Id::Profile(i) => PROFILES[*i].0.into(),
        }
    }

    /// the icon of a node, from the icons the windows already have.
    fn icon(id: &Id) -> Option<&'static str> {
        match id {
            Id::Root => Some("hosting"),
            Id::Server(_) => Some("nt4-domain"),
            Id::Servers | Id::Folder(..) | Id::Profiles | Id::Services => Some("folder"),
            Id::Gateway | Id::Http | Id::Forwards => Some("gateway"),
            Id::Site(..) | Id::Profile(_) => None,
        }
    }

    /// the nodes that show, top to bottom, with their depth.
    fn visible(&self) -> Vec<(Id, u8)> {
        let mut out = Vec::new();
        self.walk(&Id::Root, 0, &mut out);
        out
    }

    fn walk(&self, id: &Id, depth: u8, out: &mut Vec<(Id, u8)>) {
        out.push((id.clone(), depth));
        if self.open.contains(id) {
            for child in self.children(id) {
                self.walk(&child, depth + 1, out);
            }
        }
    }

    fn tree(&self) -> Node<Msg> {
        let shown = self.visible();
        let rows = shown
            .iter()
            .map(|(id, depth)| TreeRow {
                depth: *depth,
                label: self.label(id),
                icon: Self::icon(id),
                open: (!self.children(id).is_empty()).then(|| self.open.contains(id)),
            })
            .collect();
        Node::Tree(Tree {
            id: "hosting-tree",
            rows,
            selected: shown.iter().position(|(id, _)| *id == self.selected),
            height: 300.0,
            select: Msg::Select,
            open: Msg::Open,
        })
    }

    /// the machines whose kept facts are still to be read: the ones the picked node is about,
    /// and the open servers.
    fn wanted(&self) -> Vec<String> {
        if !self.can_read_facts || self.fetching.is_some() || self.remote.data.is_none() {
            return Vec::new();
        }
        let mut machines: Vec<&String> = match &self.selected {
            Id::Servers => self.names().iter().collect(),
            Id::Server(m) | Id::Folder(m, _) | Id::Site(m, ..) => {
                self.names().iter().filter(|n| *n == m).collect()
            }
            _ => Vec::new(),
        };
        machines.extend(
            self.names()
                .iter()
                .filter(|n| self.open.contains(&Id::Server((*n).clone()))),
        );
        let mut wanted: Vec<String> = Vec::new();
        for m in machines {
            if !self.facts.contains_key(m) && !wanted.contains(m) {
                wanted.push(m.clone());
            }
        }
        wanted
    }

    fn read(&mut self, services: &Services, host: &str) {
        let machines = self.wanted();
        if machines.is_empty() || services.busy(host) {
            return;
        }
        let (target, names) = (host.to_string(), machines.clone());
        let job = services.spawn(host, move |gateway, ask| {
            let mut read: Read = Vec::new();
            for name in &names {
                let facts = gateway
                    .hosting_facts(&target, name, ask)
                    .map_err(|e| e.to_string());
                read.push((name.clone(), facts));
            }
            Ok(read)
        });
        self.fetching = job.map(|job| Fetch {
            job,
            host: host.to_string(),
            machines,
        });
    }

    pub fn poll(&mut self) {
        if self.remote.poll() && self.remote.source() != self.shown_host.as_deref() {
            self.reset();
        }
        let Some(fetch) = self.fetching.take() else {
            return;
        };
        let Some(result) = fetch.job.poll() else {
            self.fetching = Some(fetch);
            return;
        };
        if self.shown_host.as_deref() != Some(fetch.host.as_str()) {
            return;
        }
        match result {
            Ok(read) => {
                for (machine, facts) in read {
                    let kept = facts.map_or_else(Kept::Failed, Kept::Facts);
                    self.facts.insert(machine, kept);
                }
            }
            Err(error) => {
                for machine in fetch.machines {
                    self.facts.insert(machine, Kept::Failed(error.clone()));
                }
            }
        }
    }

    /// nothing carries over from another host.
    fn reset(&mut self) {
        *self = Self {
            can_read_facts: self.can_read_facts,
            ..Self::default()
        };
    }

    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        if host != self.shown_host.as_deref() {
            self.reset();
            self.shown_host = host.map(str::to_string);
        }
        if let Some(host) = host {
            if self.remote.wants_load(host, false) {
                let target = host.to_string();
                let job = services.spawn(host, move |gateway, ask| {
                    crate::computers_panel::load(gateway, &target, ask)
                });
                self.remote
                    .track(host, job, "loading...", "loaded the machines", true);
            }
            self.read(services, host);
        }
        self.preview(services, host)
    }

    /// the page as it is, without asking the server for anything.
    pub fn preview(&self, _services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        if host.is_none() {
            return crate::view::no_host();
        }
        let mut nodes = vec![
            Node::Columns(vec![
                (Some(TREE_WIDTH), vec![self.tree()]),
                (None, self.details()),
            ]),
            Node::Gap(4.0),
        ];
        nodes.push(crate::view::status(&self.remote));
        nodes
    }

    /// the details list of the selected node.
    fn details(&self) -> Vec<Node<Msg>> {
        let id = &self.selected;
        let mut nodes = vec![Node::Heading(self.label(id))];
        match id {
            Id::Services => nodes.push(Node::Text(NO_SERVICES.into())),
            Id::Servers => nodes.push(self.servers_list()),
            Id::Server(m) => nodes.extend(self.server_details(m)),
            Id::Folder(m, kind) => nodes.push(self.sites_list(m, *kind)),
            Id::Site(m, kind, n) => nodes.push(self.site_properties(m, *kind, *n)),
            Id::Http => nodes.extend(placeholder(
                "Web services are published through Caddy on the gateway.",
                "Nothing is published from here yet.",
            )),
            Id::Forwards => nodes.extend(placeholder(
                "Services that are not web (FTP, SMTP) are published with port forwards.",
                "No port forwards are managed from here yet.",
            )),
            Id::Profile(i) => nodes.push(Node::Text(PROFILES[*i].1.into())),
            Id::Root | Id::Gateway | Id::Profiles => nodes.push(self.child_list(id)),
        }
        nodes
    }

    /// the children of a node as a list of name and what it is.
    fn child_list(&self, id: &Id) -> Node<Msg> {
        let rows = self
            .children(id)
            .iter()
            .map(|child| vec![self.label(child), self.blurb(child)])
            .collect();
        list(
            "hosting-children",
            vec!["Name", "Description"],
            vec![130.0],
            rows,
        )
    }

    fn blurb(&self, id: &Id) -> String {
        match id {
            Id::Services => "What you host, defined once".into(),
            Id::Servers => "The machines that host it".into(),
            Id::Gateway => "How it is published".into(),
            Id::Profiles => "The web technologies".into(),
            Id::Http => "Web sites, through Caddy".into(),
            Id::Forwards => "Everything that is not web".into(),
            Id::Profile(i) => PROFILES[*i].1.into(),
            _ => String::new(),
        }
    }

    /// what the gateway kept for a machine, in a word for a list.
    fn state_of(&self, machine: &str) -> Option<&'static str> {
        match self.facts.get(machine) {
            None => Some("reading..."),
            Some(Kept::Failed(_)) => Some("unreadable"),
            Some(Kept::Facts(None)) => Some("not detected"),
            Some(Kept::Facts(Some(_))) => None,
        }
    }

    fn servers_list(&self) -> Node<Msg> {
        if self.names().is_empty() {
            return Node::Text(
                "No hosting machines yet. Add them in the NT4 domain window.".into(),
            );
        }
        let rows = self
            .names()
            .iter()
            .map(|m| match (self.facts_of(m), self.state_of(m)) {
                (Some(f), _) => vec![
                    m.clone(),
                    if f.iis_seen() { "IIS 4" } else { "unknown" }.into(),
                    f.sites.len().to_string(),
                    when(f.detected_at),
                ],
                (None, state) => vec![
                    m.clone(),
                    state.unwrap_or_default().into(),
                    String::new(),
                    String::new(),
                ],
            })
            .collect();
        list(
            "hosting-servers",
            vec!["Name", "Platform", "Sites", "Last detected"],
            vec![100.0, 90.0, 50.0],
            rows,
        )
    }

    fn server_details(&self, machine: &str) -> Vec<Node<Msg>> {
        let mut nodes = Vec::new();
        let rows = [SiteKind::Web, SiteKind::Ftp, SiteKind::Smtp]
            .into_iter()
            .map(|kind| {
                vec![
                    folder_name(kind).to_string(),
                    self.sites(machine, kind).len().to_string(),
                ]
            })
            .collect();
        nodes.push(list(
            "hosting-folders",
            vec!["Name", "Sites"],
            vec![130.0],
            rows,
        ));
        nodes.push(Node::Gap(4.0));
        let summary = match (self.facts.get(machine), self.facts_of(machine)) {
            (_, Some(facts)) => summary(facts),
            (Some(Kept::Failed(error)), _) => vec![Node::Warning(error.clone())],
            (Some(Kept::Facts(_)), _) => vec![Node::Text(
                "Nothing detected yet. Detect runs from the debug window.".into(),
            )],
            (None, _) if !self.can_read_facts => {
                vec![Node::Text("Reading facts needs rights on Hosting.".into())]
            }
            (None, _) => vec![Node::Text("reading...".into())],
        };
        nodes.push(Node::Group {
            title: "Summary".into(),
            body: summary,
        });
        nodes
    }

    fn sites_list(&self, machine: &str, kind: SiteKind) -> Node<Msg> {
        let rows = self
            .sites(machine, kind)
            .iter()
            .map(|s| {
                vec![
                    site_name(s),
                    s.state.clone(),
                    bindings(s).join(" "),
                    s.home.clone(),
                ]
            })
            .collect();
        list(
            "hosting-sites",
            vec!["Description", "State", "Binding(s)", "Home directory"],
            vec![110.0, 60.0, 120.0],
            rows,
        )
    }

    fn site_properties(&self, machine: &str, kind: SiteKind, id: u32) -> Node<Msg> {
        let Some(site) = self.sites(machine, kind).into_iter().find(|s| s.id == id) else {
            return Node::Text("This site is no longer there.".into());
        };
        let mut rows = vec![
            vec!["Description".to_string(), site_name(site)],
            vec!["Type".into(), kind.label().into()],
            vec!["Identifier".into(), site.id.to_string()],
            vec!["State".into(), site.state.clone()],
        ];
        rows.extend(
            bindings(site)
                .into_iter()
                .map(|b| vec!["Binding".to_string(), b]),
        );
        rows.push(vec!["Home directory".into(), site.home.clone()]);
        rows.push(vec![
            "Password set".into(),
            if site.has_password { "yes" } else { "no" }.into(),
        ]);
        plain("hosting-site", vec!["Property", "Value"], vec![110.0], rows)
    }

    pub fn update(&mut self, msg: Msg, _services: &Services, _host: &str) {
        match msg {
            Msg::Select(row) => {
                if let Some((id, _)) = self.visible().get(row) {
                    self.select(id.clone());
                }
            }
            Msg::Open(row, open) => self.set_open(row, open),
            Msg::Child(n) => {
                if let Some(child) = self.children(&self.selected).get(n).cloned() {
                    self.open.insert(self.selected.clone());
                    self.select(child);
                }
            }
            Msg::Nothing => {}
        }
    }

    /// pick a node. a server or the list of them is read again from what the gateway kept.
    fn select(&mut self, id: Id) {
        match &id {
            Id::Servers => self.facts.clear(),
            Id::Server(m) => {
                self.facts.remove(m);
            }
            _ => {}
        }
        self.selected = id;
    }

    fn set_open(&mut self, row: usize, open: bool) {
        let shown = self.visible();
        let Some((id, depth)) = shown.get(row).cloned() else {
            return;
        };
        if open {
            self.open.insert(id);
            return;
        }
        // closing a branch that holds the picked node picks the branch
        let inside = shown[row + 1..]
            .iter()
            .take_while(|(_, d)| *d > depth)
            .any(|(child, _)| *child == self.selected);
        self.open.remove(&id);
        if inside {
            self.select(id);
        }
    }

    /// the same box the debug window has for a run: this window has none.
    pub fn dialog(&mut self) -> Option<Dialog<()>> {
        None
    }

    pub fn update_dialog(&mut self, _msg: (), _services: &Services, _host: Option<&str>) {}

    /// the picked node's name and its open branches' names, for tests.
    pub fn selected_label(&self) -> String {
        self.label(&self.selected)
    }

    /// the labels of the rows showing, for tests.
    pub fn shown_labels(&self) -> Vec<String> {
        self.visible()
            .iter()
            .map(|(id, _)| self.label(id))
            .collect()
    }

    /// the machine names, for tests.
    pub fn names_shown(&self) -> Vec<String> {
        self.names().to_vec()
    }
}

/// two lines that say what will live in a node.
fn placeholder(what: &str, state: &str) -> Vec<Node<Msg>> {
    vec![
        Node::Text(what.into()),
        Node::Gap(4.0),
        Node::Text(state.into()),
    ]
}

/// the facts of a machine as name and value lines.
fn summary(facts: &HostingFacts) -> Vec<Node<Msg>> {
    let running = facts.services.iter().filter(|s| s.running).count();
    let checks = if facts.checks.is_empty() {
        "none kept".to_string()
    } else {
        let (fail, warn) = facts.check_counts();
        format!("{fail} fail, {warn} warn")
    };
    let mut nodes = vec![
        Node::field(
            "IIS",
            if facts.iis_seen() {
                "present"
            } else {
                "not present"
            },
        ),
        Node::field("Services running", running.to_string()),
        Node::field("Hardening checks", checks),
        Node::field("Last detected", when(facts.detected_at)),
    ];
    nodes.extend(facts.notes.iter().cloned().map(Node::Text));
    nodes
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::ask::never;
    use crate::gateway::{Fake, Gateway};
    use crate::wake::Wake;

    fn tree() -> (HostingTree, Services, Arc<Fake>) {
        let fake = Arc::new(Fake::new());
        *fake.machines.lock().unwrap() = vec!["NT4DEV02".into(), "NT4DEV01".into()];
        let services = Services::new(fake.clone(), Wake::none());
        let mut tree = HostingTree::default();
        tree.set_can_read_facts(true);
        tree.view(&services, Some("a@b"));
        wait(&mut tree, &services);
        (tree, services, fake)
    }

    fn wait(tree: &mut HostingTree, services: &Services) {
        let start = Instant::now();
        loop {
            tree.poll();
            tree.view(services, Some("a@b"));
            if !tree.remote.busy() && tree.fetching.is_none() && tree.wanted().is_empty() {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// every piece of text the nodes show, joined by spaces.
    fn text_of(nodes: &[Node<Msg>]) -> String {
        let mut out = Vec::new();
        for node in nodes {
            match node {
                Node::Text(t) | Node::Heading(t) | Node::Bullet(t) => out.push(t.clone()),
                Node::Field { label, value } => out.push(format!("{label} {value}")),
                Node::Group { body, .. } => out.push(text_of(body)),
                Node::Table(t) => out.extend(t.rows.iter().map(|r| r.join(" "))),
                _ => {}
            }
        }
        out.join(" ")
    }

    fn click(tree: &mut HostingTree, services: &Services, label: &str) {
        let at = tree
            .shown_labels()
            .iter()
            .position(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no row {label}"));
        tree.update(Msg::Select(at), services, "a@b");
        wait(tree, services);
    }

    #[test]
    fn it_starts_with_the_root_and_its_folders() {
        let (tree, _, _) = tree();
        assert_eq!(
            tree.shown_labels(),
            [
                "LANlord Hosting",
                "Services",
                "Servers",
                "NT4DEV01",
                "NT4DEV02",
                "Gateway",
                "Profiles"
            ]
        );
    }

    #[test]
    fn the_services_node_says_why_it_is_empty() {
        let (mut tree, services, _) = tree();
        click(&mut tree, &services, "Services");
        let text = text_of(&tree.details());
        assert!(text.contains("No services yet"));
    }

    #[test]
    fn a_detected_server_shows_its_sites_and_check_counts() {
        let (mut tree, services, fake) = tree();
        fake.hosting_detect("a@b", "NT4DEV01", &mut never).unwrap();
        click(&mut tree, &services, "NT4DEV01");
        let text = text_of(&tree.details());
        assert!(text.contains("1 fail, 1 warn"), "{text}");
        tree.update(Msg::Child(0), &services, "a@b");
        wait(&mut tree, &services);
        assert!(tree.shown_labels().iter().any(|l| l == "Web Sites"));
        click(&mut tree, &services, "Web Sites");
        let text = text_of(&tree.details());
        assert!(text.contains("Intranet"), "{text}");
    }

    #[test]
    fn nothing_is_read_without_the_right_to() {
        let fake = Arc::new(Fake::new());
        *fake.machines.lock().unwrap() = vec!["NT4DEV01".into()];
        let services = Services::new(fake.clone(), Wake::none());
        let mut tree = HostingTree::default();
        tree.view(&services, Some("a@b"));
        click(&mut tree, &services, "Servers");
        assert!(tree.wanted().is_empty());
        assert!(!fake.calls().iter().any(|l| l.contains("hosting-facts")));
    }

    #[test]
    fn closing_a_branch_that_holds_the_pick_picks_the_branch() {
        let (mut tree, services, _) = tree();
        click(&mut tree, &services, "NT4DEV01");
        let servers = tree
            .shown_labels()
            .iter()
            .position(|l| l == "Servers")
            .unwrap();
        tree.update(Msg::Open(servers, false), &services, "a@b");
        assert_eq!(tree.selected_label(), "Servers");
        assert!(!tree.shown_labels().iter().any(|l| l == "NT4DEV01"));
    }
}
