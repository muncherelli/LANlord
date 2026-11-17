//! the frame of every module window (its icon, name, status and summary) and the window of a
//! module that has nothing to edit yet.

use lanlord_model::modules::{Module, Status};

use crate::services::Services;
use crate::view::{Button, Node};

/// what a click in a plain module window means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Apply,
}

/// the top of a module window, above whatever the module shows.
pub fn header<M>(module: &Module) -> Vec<Node<M>> {
    let status = match module.status {
        Status::Available => "available",
        Status::Planned => "planned - not built yet",
    };
    vec![
        Node::Header {
            icon: module.id,
            title: module.name.into(),
            lines: vec![format!("Status: {status}")],
        },
        Node::Gap(2.0),
        Node::Text(module.summary.into()),
        Node::Gap(4.0),
        Node::Separator,
        Node::Gap(4.0),
    ]
}

/// a module with nothing to edit yet: its example settings and an Apply button.
pub fn plain(module: &Module, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
    let busy = host.is_some_and(|h| services.busy(h));
    let ready = module.status == Status::Available && !busy && host.is_some();
    let status = match (host, busy) {
        (None, _) => "No host selected".to_string(),
        (Some(host), true) => format!("Host: {host} - working..."),
        (Some(host), false) => format!("Host: {host}"),
    };
    vec![
        Node::Group {
            title: "Settings (in your settings file)".into(),
            body: vec![Node::Code(module.example.into())],
        },
        Node::Gap(6.0),
        Node::Buttons(vec![Button::new("Apply", ready, Msg::Apply)]),
        Node::Status {
            text: status,
            error: false,
        },
    ]
}

/// act on a click in a plain module window.
pub fn update(msg: Msg, module: &Module, services: &Services, host: Option<&str>) {
    match msg {
        Msg::Apply => {
            if let Some(host) = host {
                services.apply(host, vec![module.id.to_string()]);
            }
        }
    }
}
