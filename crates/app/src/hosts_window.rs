//! the Hosts window: the gateways a client manages and which one every action targets.

use lanlord_core::log::{emit, Kind};

use crate::hostlist::HostList;
use crate::services::Services;
use crate::view::{Button, Node};

/// what a click in the window means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Pick(usize),
    Add,
    Remove,
    Bootstrap,
}

/// what the window needs the front end to do for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// ask for a new `user@host`, then pass it to `HostList::add`.
    AddHost,
}

/// the window for `hosts`; `local` is lanlord on the gateway itself, which has no list.
pub fn view(hosts: &HostList, services: &Services, local: bool) -> Vec<Node<Msg>> {
    if local {
        return vec![Node::Text(
            "This computer is the server; there are no other hosts to pick.".into(),
        )];
    }
    let busy = hosts.selected().is_some_and(|h| services.busy(h));
    let ready = hosts.selected().is_some() && !busy;
    vec![
        Node::Group {
            title: "Gateways lanlord manages".into(),
            body: vec![Node::List {
                id: "hosts",
                items: hosts.hosts().to_vec(),
                selected: Some(hosts.selected_index()),
                min_height: 100.0,
                empty: "No gateways yet. Click Add... and type user@gateway.".into(),
                select: Msg::Pick,
            }],
        },
        Node::Gap(6.0),
        Node::Toolbar(vec![
            Button::new("Add...", true, Msg::Add),
            Button::new("Remove", ready, Msg::Remove),
            Button::new("Bootstrap", ready, Msg::Bootstrap),
        ]),
        Node::Gap(6.0),
        Node::Status {
            text: if busy { "working..." } else { "ready" }.into(),
            error: false,
        },
    ]
}

/// act on a click; what the front end must do next, if anything.
pub fn update(msg: Msg, hosts: &mut HostList, services: &Services) -> Option<Ask> {
    match msg {
        Msg::Pick(i) => hosts.select(i),
        Msg::Add => return Some(Ask::AddHost),
        Msg::Remove => {
            if let Err(error) = hosts.remove_selected() {
                emit(Kind::Err, error);
            }
        }
        Msg::Bootstrap => {
            if let Some(host) = hosts.selected() {
                services.bootstrap(host);
            }
        }
    }
    None
}
