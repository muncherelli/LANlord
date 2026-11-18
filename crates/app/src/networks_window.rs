//! the Networks window: the server's external network and the networks lanlord runs, in the
//! style of Network and Dial-up Connections. each network has a tab showing what Windows shows
//! in a connection's Status box (link, speed and traffic), and Details... opens the Network
//! Connection Details list. it only shows them in this version.

use lanlord_model::netstatus::InterfaceStatus;
use lanlord_model::networks::{self, NetworkRow, Role};
use lanlord_model::settings::Settings;

use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Table};

/// how often an open window reads the status again, so the traffic counts move.
pub const STATUS_EVERY: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    /// the tab of this network.
    Tab(usize),
    Refresh,
    /// open the Network Connection Details of the tab that is open.
    Details,
    /// open the DNS & DHCP window.
    Dns,
    /// open the properties of the tab that is open.
    Properties,
    CloseProperties,
    CloseConnection,
    /// a click that means nothing, such as one on a row of a read-only list.
    Ignore,
}

/// what the window asks the desktop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// open the DNS & DHCP window.
    Dns,
}

#[derive(Default)]
pub struct NetworksWindow {
    rows: Remote<Vec<NetworkRow>>,
    /// the status of every connection, matched to a row by its interface.
    status: Remote<Vec<InterfaceStatus>>,
    /// the tab that is open.
    selected: usize,
    /// the network whose properties are open.
    details: Option<NetworkRow>,
    /// the connection whose Network Connection Details are open.
    connection: Option<InterfaceStatus>,
    /// when the status was last asked for, in seconds on the caller's clock.
    status_at: f64,
}

impl NetworksWindow {
    pub fn rows(&self) -> &Remote<Vec<NetworkRow>> {
        &self.rows
    }

    pub fn status(&self) -> &Remote<Vec<InterfaceStatus>> {
        &self.status
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn details(&self) -> Option<&NetworkRow> {
        self.details.as_ref()
    }

    pub fn connection(&self) -> Option<&InterfaceStatus> {
        self.connection.as_ref()
    }

    pub fn poll(&mut self) {
        self.rows.poll();
        self.status.poll();
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            let facts = gateway.server_facts(&target, ask)?;
            // networks still show without settings, or without the key to read them
            let settings = gateway
                .load_settings(&target, ask)
                .unwrap_or_else(|_| Settings::empty());
            let leases = gateway.leases(&target, ask).ok().map(|l| l.len());
            Ok(networks::rows(&facts.interfaces, &settings, leases))
        });
        self.rows
            .track(host, job, "reading the networks...", "up to date", true);
    }

    fn reload_status(&mut self, services: &Services, host: &str, now: f64) {
        let target = host.to_string();
        let job = services.quiet(move |gateway| gateway.network_status(&target));
        self.status_at = now;
        self.status.track(host, Some(job), "", "", true);
    }

    fn status_error(&self) -> Option<String> {
        self.status
            .note
            .as_ref()
            .filter(|(error, _)| *error)
            .map(|(_, text)| text.clone())
    }

    /// the status loads when the window opens, then again every few seconds while it is open
    /// and the last read worked; after a failure it waits for Refresh.
    fn refresh(&mut self, services: &Services, host: &str, now: f64) {
        self.poll();
        if self.rows.wants_load(host, false) {
            self.reload(services, host);
        }
        let stale = self.status.belongs_to(host)
            && self.status_error().is_none()
            && !self.status.busy()
            && now - self.status_at >= STATUS_EVERY;
        if self.status.wants_load(host, false) || stale {
            self.reload_status(services, host, now);
        }
    }

    fn chosen(&self) -> Option<&NetworkRow> {
        let rows = self.rows.data.as_ref()?;
        rows.get(self.selected.min(rows.len().saturating_sub(1)))
    }

    fn status_of(&self, row: &NetworkRow) -> Option<&InterfaceStatus> {
        self.status
            .data
            .as_ref()?
            .iter()
            .find(|s| s.name == row.interface)
    }

    fn reading(&self) -> bool {
        self.rows.busy() || self.status.busy()
    }

    pub fn view(&mut self, services: &Services, host: Option<&str>, now: f64) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text("No server yet. Sign in to one first.".into())];
        };
        self.refresh(services, host, now);
        let rows = self.rows.data.clone().unwrap_or_default();
        if rows.is_empty() {
            return vec![view::status(&self.rows)];
        }
        self.selected = self.selected.min(rows.len() - 1);
        let titles = rows.iter().map(|r| r.name.clone()).collect();
        let error = self.status_error();
        let chosen = &rows[self.selected];
        let status = self.status_of(chosen);
        let lanlord = chosen.role == Role::Lanlord;
        let tabs = Node::Tabs {
            titles,
            current: self.selected,
            pick: Msg::Tab,
            body: page(chosen, status, error.as_deref()),
        };
        let buttons = Node::Buttons(vec![
            Button::new("Refresh", !self.reading(), Msg::Refresh),
            Button::new("Details...", status.is_some(), Msg::Details),
            Button::new("DNS & DHCP...", lanlord && !services.busy(host), Msg::Dns),
            Button::new("Properties...", true, Msg::Properties),
        ]);
        vec![
            tabs,
            Node::Gap(6.0),
            buttons,
            Node::Gap(4.0),
            Node::Text(
                "Editing networks comes next. For now you can look at them, and open DNS & DHCP."
                    .into(),
            ),
            Node::Gap(4.0),
            view::status(&self.rows),
        ]
    }

    pub fn update(&mut self, msg: Msg, services: &Services, host: &str, now: f64) -> Option<Ask> {
        match msg {
            Msg::Tab(i) => self.selected = i,
            Msg::Refresh if !self.reading() => {
                self.reload(services, host);
                self.reload_status(services, host, now);
            }
            Msg::Details => {
                self.connection = self.chosen().and_then(|r| self.status_of(r)).cloned();
            }
            Msg::Dns => {
                let lanlord = self.chosen().is_some_and(|r| r.role == Role::Lanlord);
                if lanlord && !services.busy(host) {
                    return Some(Ask::Dns);
                }
            }
            Msg::Properties => self.details = self.chosen().cloned(),
            Msg::CloseProperties => self.details = None,
            Msg::CloseConnection => self.connection = None,
            Msg::Refresh | Msg::Ignore => {}
        }
        None
    }

    /// the read-only properties box and the Network Connection Details list.
    pub fn dialog(&self) -> Option<Dialog<Msg>> {
        if let Some(status) = &self.connection {
            return Some(connection_dialog(status));
        }
        self.details.as_ref().map(properties_dialog)
    }
}

fn properties_dialog(row: &NetworkRow) -> Dialog<Msg> {
    let mut body: Vec<Node<Msg>> = row
        .details()
        .into_iter()
        .map(|(label, value)| Node::field(&label, value))
        .collect();
    body.push(Node::Gap(6.0));
    body.push(Node::Buttons(vec![Button::new(
        "OK",
        true,
        Msg::CloseProperties,
    )]));
    Dialog {
        id: "network-details",
        title: format!("{} Properties", row.name),
        body,
        escape: Some(Msg::CloseProperties),
    }
}

fn connection_dialog(status: &InterfaceStatus) -> Dialog<Msg> {
    let rows = status
        .details(local_time)
        .into_iter()
        .map(|(key, value)| vec![key, value])
        .collect();
    Dialog {
        id: "network-connection-details",
        title: "Network Connection Details".into(),
        body: vec![
            Node::Text("Network Connection Details:".into()),
            Node::Gap(4.0),
            Node::Table(Table {
                id: "network-details-list",
                headings: vec!["Property", "Value"],
                widths: vec![150.0],
                rows,
                selected: None,
                bad: Vec::new(),
                height: 220.0,
                select: |_| Msg::Ignore,
            }),
            Node::Gap(6.0),
            Node::Buttons(vec![Button::new("Close", true, Msg::CloseConnection)]),
        ],
        escape: Some(Msg::CloseConnection),
    }
}

/// one network's tab: Connection and Activity like a connection's Status box, and the lanlord
/// network's dhcp.
fn page(row: &NetworkRow, status: Option<&InterfaceStatus>, error: Option<&str>) -> Vec<Node<Msg>> {
    let state = status.map_or(row.link.clone(), |s| s.connection.text().to_string());
    let mut nodes = vec![Node::Group {
        title: "Connection".into(),
        body: vec![
            Node::field("Status:", state),
            Node::field("Interface:", row.interface.clone()),
            Node::field(
                "Speed:",
                status.map_or("Unknown".to_string(), InterfaceStatus::speed),
            ),
            Node::field("Address:", row.address.clone()),
        ],
    }];
    if let (None, Some(error)) = (status, error) {
        nodes.push(Node::Gap(6.0));
        nodes.push(Node::Text(format!(
            "Could not read the connection status: {error}. Press Refresh to try again."
        )));
    }
    if let Some(status) = status {
        nodes.push(Node::Gap(6.0));
        nodes.push(activity(status));
    }
    if row.role == Role::Lanlord {
        nodes.push(Node::Gap(6.0));
        nodes.push(lanlord_network(row));
    }
    nodes
}

fn activity(status: &InterfaceStatus) -> Node<Msg> {
    let c = status.counters;
    Node::Group {
        title: "Activity".into(),
        body: vec![Node::Table(Table {
            id: "network-activity",
            headings: vec!["", "Sent", "Received"],
            widths: vec![80.0, 160.0],
            rows: vec![
                vec![
                    "Bytes:".into(),
                    grouped(c.sent_bytes),
                    grouped(c.received_bytes),
                ],
                vec![
                    "Packets:".into(),
                    grouped(c.sent_packets),
                    grouped(c.received_packets),
                ],
            ],
            selected: None,
            bad: Vec::new(),
            height: 44.0,
            select: |_| Msg::Ignore,
        })],
    }
}

fn lanlord_network(row: &NetworkRow) -> Node<Msg> {
    let dhcp = row.dhcp.clone().unwrap_or_else(|| "not served".into());
    let clients = row.leases.map_or("-".to_string(), |n| n.to_string());
    Node::Group {
        title: "LANlord network".into(),
        body: vec![
            Node::field("DHCP range:", dhcp),
            Node::field("Clients with a lease:", clients),
        ],
    }
}

/// a count with thousands separators, the way Windows writes traffic.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// a unix time in this computer's time zone, like `11/16/2026 4:05:09 PM`.
fn local_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-m/%-d/%Y %-I:%M:%S %p")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::server::demo;

    #[test]
    fn rows_name_one_tab_per_network() {
        let settings = Settings::parse(
            "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n",
        )
        .unwrap();
        let rows = networks::rows(&demo().interfaces, &settings, Some(2));
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["External Network", "retronet"]);
        assert_eq!(
            rows[1].dhcp.as_deref(),
            Some("172.16.120.100 - 172.16.120.200")
        );
    }

    #[test]
    fn traffic_counts_are_grouped_in_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(913_442_071), "913,442,071");
    }

    #[test]
    fn a_bad_time_is_blank_not_a_panic() {
        assert_eq!(local_time(i64::MAX), "");
        assert!(!local_time(1_795_000_000).is_empty());
    }

    #[test]
    fn without_a_server_it_says_so_and_buttons_do_nothing() {
        let services = Services::new(
            std::sync::Arc::new(crate::gateway::Fake::new()),
            crate::wake::Wake::none(),
        );
        let mut window = NetworksWindow::default();
        let nodes = window.view(&services, None, 0.0);
        assert!(matches!(&nodes[..], [Node::Text(text)] if text.starts_with("No server")));
        assert!(window.update(Msg::Refresh, &services, "h", 0.0).is_none());
        assert!(window.update(Msg::Dns, &services, "h", 0.0).is_none());
        assert!(window.dialog().is_none());
        window.update(Msg::Properties, &services, "h", 0.0);
        window.update(Msg::CloseProperties, &services, "h", 0.0);
        assert!(window.dialog().is_none());
    }
}
