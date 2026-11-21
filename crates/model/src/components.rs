//! the components a LANlord host can run, which of them this host has switched on, and which
//! networks each one joins.
//!
//! the catalogue is built from `modules::MODULES`, so a module's name, icon and summary are
//! written once. a host's choice lives in the `components` section of the settings:
//!
//! ```yaml
//! components:
//!   nt4-domain:
//!     enabled: true
//!     subcomponents: [domain-controller, wins]
//!     attach:
//!       - {network: retronet, address: 192.0.2.5}   # its own address there
//!       - {network: lab}                            # no address: the gateway's address
//!     mechanism: host-bind                          # the default
//!   web-proxy:
//!     attach: [{network: retronet}, {network: external}]
//! ```
//!
//! an entry without `enabled` is switched on. an entry keeps whatever other keys it has; this
//! code only reads and writes `enabled`, `subcomponents`, `attach` and `mechanism`. settings
//! written before this section existed are read the old way: every module with a top-level
//! section of its own is switched on, joined to the one internal network, with
//! `nt4-domain.address` and `web-proxy.listen` turned into attachments.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use serde_yaml_ng::{Mapping, Value};

use crate::modules::{Module, MODULES};
use crate::settings::{Settings, Subnet, ValidationError};

/// the settings key the choice is stored under.
pub const SECTION: &str = "components";

/// the id of the internal network in settings that predate the `networks` list.
pub const LEGACY_NETWORK: &str = "internal";

/// the id of the gateway's normal (outside) network when the settings name none.
pub const EXTERNAL_NETWORK: &str = "external";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Proto {
    Tcp,
    Udp,
}

/// a service a component offers to a network; one provider of each per network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Role {
    Wins,
    Dns,
    Dhcp,
    LogonServer,
    Http,
    Ntp,
}

/// how a component gets onto a network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mechanism {
    /// the gateway carries the address and the service binds it (the default).
    #[default]
    HostBind,
    /// its own network namespace on an ipvlan of the network's bridge (opt-in).
    Ipvlan,
    /// like ipvlan with a mac address of its own; only for things that need one.
    Macvlan,
}

impl Mechanism {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// an optional piece of a component, shown under Details.
#[derive(Debug, serde::Serialize)]
pub struct SubComponent {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// switched on when the component is first chosen.
    pub default: bool,
}

/// one entry in the catalogue.
#[derive(Debug, serde::Serialize)]
pub struct Component {
    pub module: &'static Module,
    /// always installed; the box cannot be cleared.
    pub required: bool,
    pub requires_docker: bool,
    pub ports: &'static [(Proto, u16)],
    pub roles: &'static [Role],
    /// speaks netbios: at most one such component per network.
    pub netbios: bool,
    /// how many networks it joins: at least `attach_min`, at most `attach_max` (0 = none).
    pub attach_min: usize,
    pub attach_max: Option<usize>,
    /// it may also join the gateway's outside network.
    pub external: bool,
    /// it must have an address no other component uses.
    pub needs_own_address: bool,
    /// the mechanisms it supports; the first is its default.
    pub mechanisms: &'static [Mechanism],
    pub subcomponents: &'static [SubComponent],
}

impl Component {
    pub fn id(&self) -> &'static str {
        self.module.id
    }

    pub fn name(&self) -> &'static str {
        self.module.name
    }

    pub fn description(&self) -> &'static str {
        self.module.summary
    }

    /// does it join networks at all?
    pub fn attaches(&self) -> bool {
        self.attach_max != Some(0)
    }

    /// can it have an address of its own on a network (rather than only the gateway's)?
    pub fn own_address(&self) -> bool {
        self.attaches()
    }

    /// the short text in the list's right-hand column, like "Docker, 1+ network".
    pub fn needs(&self) -> String {
        let mut needs = Vec::new();
        if self.requires_docker {
            needs.push("Docker".to_string());
        }
        if self.attach_min > 0 {
            needs.push(format!("{}+ network", self.attach_min));
        } else if self.attaches() {
            needs.push("networks optional".into());
        }
        if needs.is_empty() {
            "gateway only".into()
        } else {
            needs.join(", ")
        }
    }

    /// the ports it listens on, like "tcp 53, udp 53".
    pub fn ports(&self) -> String {
        self.ports
            .iter()
            .map(|(proto, port)| {
                let proto = match proto {
                    Proto::Tcp => "tcp",
                    Proto::Udp => "udp",
                };
                format!("{proto} {port}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// the ids of the subcomponents switched on by default.
    pub fn default_subcomponents(&self) -> Vec<String> {
        self.subcomponents
            .iter()
            .filter(|p| p.default)
            .map(|p| p.id.to_string())
            .collect()
    }
}

/// what the catalogue adds to a module.
fn component(module: &'static Module) -> Component {
    use Proto::{Tcp, Udp};
    let plain = Component {
        module,
        required: false,
        requires_docker: false,
        ports: &[],
        roles: &[],
        netbios: false,
        attach_min: 0,
        attach_max: Some(0),
        external: false,
        needs_own_address: false,
        mechanisms: &[Mechanism::HostBind],
        subcomponents: &[],
    };
    match module.id {
        "gateway" => Component {
            required: true,
            ..plain
        },
        "nt4-domain" => Component {
            requires_docker: true,
            ports: &[(Udp, 137), (Udp, 138), (Tcp, 139), (Tcp, 445)],
            roles: &[Role::Wins, Role::LogonServer],
            netbios: true,
            attach_min: 1,
            attach_max: None,
            mechanisms: &[Mechanism::HostBind, Mechanism::Ipvlan],
            subcomponents: &[
                SubComponent {
                    id: "domain-controller",
                    name: "Domain controller",
                    description: "An NT4-style primary domain controller for logons.",
                    default: true,
                },
                SubComponent {
                    id: "wins",
                    name: "WINS server",
                    description:
                        "Name lookups for old Windows. Every network it joins shares one WINS list.",
                    default: true,
                },
                SubComponent {
                    id: "software-share",
                    name: "Software share",
                    description: "The S: drive with installers, mapped at logon.",
                    default: false,
                },
            ],
            ..plain
        },
        "web-proxy" => Component {
            requires_docker: true,
            ports: &[(Tcp, 80), (Tcp, 443)],
            roles: &[Role::Http],
            attach_min: 1,
            attach_max: None,
            external: true,
            mechanisms: &[Mechanism::HostBind, Mechanism::Ipvlan],
            subcomponents: &[
                SubComponent {
                    id: "sites",
                    name: "Hosted sites",
                    description: "Old-browser friendly sites proxied from the network.",
                    default: true,
                },
                SubComponent {
                    id: "status-page",
                    name: "Status page",
                    description: "A page showing what LANlord is doing.",
                    default: false,
                },
            ],
            ..plain
        },
        // the DNS server and the DHCP server are two entries that share one dnsmasq per network
        "dns" => Component {
            requires_docker: true,
            ports: &[(Udp, 53), (Tcp, 53)],
            roles: &[Role::Dns],
            attach_min: 1,
            attach_max: None,
            mechanisms: &[Mechanism::HostBind, Mechanism::Ipvlan],
            ..plain
        },
        "dhcp" => Component {
            requires_docker: true,
            ports: &[(Udp, 67)],
            roles: &[Role::Dhcp],
            attach_min: 1,
            attach_max: None,
            mechanisms: &[Mechanism::HostBind, Mechanism::Ipvlan],
            ..plain
        },
        "time-server" => Component {
            requires_docker: true,
            ports: &[(Udp, 123)],
            roles: &[Role::Ntp],
            attach_min: 1,
            attach_max: None,
            ..plain
        },
        "remote-commands" => Component {
            attach_max: None,
            ..plain
        },
        _ => plain,
    }
}

/// every component, in the order of `MODULES`.
pub fn catalogue() -> Vec<Component> {
    MODULES.iter().map(component).collect()
}

/// look a component up by its id.
pub fn find(id: &str) -> Option<Component> {
    MODULES.iter().find(|m| m.id == id).map(component)
}

/// a component joined to one network.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub network: String,
    /// its own address there; none means the gateway's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<Ipv4Addr>,
}

/// a switched-on component: the networks it joins, how, and the subcomponents it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Choice {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attach: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Mechanism::is_default")]
    pub mechanism: Mechanism,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcomponents: Vec<String>,
}

/// how a `components` entry is stored; `enabled` defaults to on.
#[derive(serde::Deserialize)]
struct Stored {
    #[serde(default = "on")]
    enabled: bool,
    #[serde(flatten)]
    choice: Choice,
}

fn on() -> bool {
    true
}

/// the components a host has switched on, by id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub enabled: BTreeMap<String, Choice>,
}

impl Selection {
    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.contains_key(id)
    }

    /// the choice the old way only, from the top-level module sections; what a front end can
    /// still show when the `components` section cannot be read.
    pub fn from_sections(settings: &Settings) -> Self {
        let networks = networks_from_settings(settings);
        let mut enabled = legacy(settings, &networks);
        split_dhcp(&mut enabled);
        for choice in enabled.values_mut() {
            normalise(choice, &networks);
        }
        for component in catalogue().iter().filter(|c| c.required) {
            enabled.entry(component.id().to_string()).or_default();
        }
        Self { enabled }
    }

    /// the choice in `settings` (see the module docs for the old way of reading it).
    pub fn from_settings(settings: &Settings) -> anyhow::Result<Self> {
        let networks = networks_from_settings(settings);
        let mut enabled = legacy(settings, &networks);
        match settings.section(SECTION) {
            None | Some(Value::Null) => {}
            Some(Value::Mapping(entries)) => {
                for (key, value) in entries {
                    let id = key
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("components: the keys must be names"))?;
                    let value = match value {
                        Value::Null => Value::Mapping(Mapping::new()),
                        other => other.clone(),
                    };
                    let stored: Stored = serde_yaml_ng::from_value(value)
                        .map_err(|e| anyhow::anyhow!("components.{id}: {e}"))?;
                    if stored.enabled {
                        enabled.insert(id.to_string(), stored.choice);
                    } else {
                        enabled.remove(id);
                    }
                }
            }
            Some(_) => anyhow::bail!("components must be a mapping of component names"),
        }
        if !matches!(settings.section(SECTION), Some(Value::Mapping(m)) if m.contains_key(DHCP)) {
            split_dhcp(&mut enabled);
        }
        for choice in enabled.values_mut() {
            normalise(choice, &networks);
        }
        for component in catalogue().iter().filter(|c| c.required) {
            enabled.entry(component.id().to_string()).or_default();
        }
        Ok(Self { enabled })
    }

    /// write the choice into `settings`. switched-on entries get `enabled: true` and their
    /// networks; switched-off ones that were there get `enabled: false`. other keys of an entry,
    /// and the old top-level module sections, stay as they are.
    pub fn apply_to(&self, settings: &mut Settings) {
        let mut entries = match settings.section(SECTION) {
            Some(Value::Mapping(map)) => map.clone(),
            _ => Mapping::new(),
        };
        let keys = ["enabled", "attach", "mechanism", "subcomponents"];
        for (key, value) in entries.iter_mut() {
            if key.as_str().is_some_and(|id| !self.is_enabled(id)) {
                if let Value::Mapping(map) = value {
                    map.insert("enabled".into(), Value::Bool(false));
                } else {
                    *value = Value::Mapping(Mapping::from_iter([(
                        Value::from("enabled"),
                        Value::Bool(false),
                    )]));
                }
            }
        }
        // a component that is only on through its old top-level section needs a written
        // `enabled: false`, or the old way of reading would switch it back on
        for component in catalogue() {
            let id = component.id();
            // `dhcp` and `dns` too: with no entry the playbook would read the old settings, and
            // a missing `dhcp` would switch it back on, a missing `dns` beside a ticked dhcp too
            let written = settings.section(id).is_some()
                || (id == DHCP && self.is_enabled("dns"))
                || (id == "dns" && self.is_enabled(DHCP));
            if !self.is_enabled(id) && written && !entries.contains_key(id) {
                entries.insert(
                    Value::String(id.into()),
                    Value::Mapping(Mapping::from_iter([(
                        Value::from("enabled"),
                        Value::Bool(false),
                    )])),
                );
            }
        }
        for (id, choice) in &self.enabled {
            let mut entry = match entries.get(id.as_str()) {
                Some(Value::Mapping(map)) => map.clone(),
                _ => Mapping::new(),
            };
            for key in keys {
                entry.remove(key);
            }
            entry.insert("enabled".into(), Value::Bool(true));
            if let Ok(Value::Mapping(fields)) = serde_yaml_ng::to_value(choice) {
                entry.extend(fields);
            }
            entries.insert(Value::String(id.clone()), Value::Mapping(entry));
        }
        settings.set_section(SECTION, Value::Mapping(entries));
    }

    /// everything wrong with this choice, given the host's networks.
    pub fn validate(&self, networks: &[Network]) -> Vec<ValidationError> {
        let catalogue = catalogue();
        let mut errors = self.required_errors(&catalogue);
        let mut claims = Claims::default();
        for (id, choice) in &self.enabled {
            let Some(component) = catalogue.iter().find(|c| c.id() == id) else {
                errors.push(fail(
                    format!("{SECTION}.{id}"),
                    format!("there is no component called {id}"),
                ));
                continue;
            };
            let site = Site {
                component,
                id,
                choice,
                path: format!("{SECTION}.{id}"),
            };
            errors.extend(self.component_errors(&site));
            if component.netbios && choice.mechanism == Mechanism::HostBind {
                claims.host_bound_netbios += 1;
            }
            errors.extend(attach_count_errors(&site));
            if component.attach_max == Some(0) && !choice.attach.is_empty() {
                continue;
            }
            for (j, attachment) in choice.attach.iter().enumerate() {
                let earlier = &choice.attach[..j];
                errors.extend(attachment_errors(
                    &site,
                    networks,
                    (j, attachment, earlier),
                    &mut claims,
                ));
            }
        }
        errors.extend(claims.shared_address_errors());
        errors.extend(claims.netbios_host_error());
        errors
    }

    /// required components that are switched off.
    fn required_errors(&self, catalogue: &[Component]) -> Vec<ValidationError> {
        catalogue
            .iter()
            .filter(|c| c.required && !self.is_enabled(c.id()))
            .map(|c| {
                fail(
                    format!("{SECTION}.{}", c.id()),
                    format!("{} cannot be removed", c.name()),
                )
            })
            .collect()
    }

    /// docker, mechanism and subcomponent problems of one entry.
    fn component_errors(&self, site: &Site) -> Vec<ValidationError> {
        let (component, choice, path) = (site.component, site.choice, &site.path);
        let docker = (component.requires_docker && !self.is_enabled("docker"))
            .then(|| fail(path.clone(), format!("{} needs Docker", component.name())));
        let mechanism = (!component.mechanisms.contains(&choice.mechanism)).then(|| {
            fail(
                format!("{path}.mechanism"),
                format!("{} cannot run as {:?}", component.name(), choice.mechanism),
            )
        });
        let parts = choice
            .subcomponents
            .iter()
            .filter(|sub| !component.subcomponents.iter().any(|p| p.id == **sub))
            .map(|sub| {
                fail(
                    format!("{path}.subcomponents"),
                    format!("{} has no part called {sub}", component.name()),
                )
            });
        docker.into_iter().chain(mechanism).chain(parts).collect()
    }
}

/// one enabled entry and the catalogue component it names.
struct Site<'a> {
    component: &'a Component,
    id: &'a str,
    choice: &'a Choice,
    path: String,
}

/// what the entries seen so far have taken, to spot clashes.
#[derive(Default)]
struct Claims<'a> {
    /// (network, address or none for the gateway's, proto, port) -> the component on it
    ports: BTreeMap<(String, Option<Ipv4Addr>, Proto, u16), &'a str>,
    /// (network, address) -> the components on it
    addresses: BTreeMap<(String, Option<Ipv4Addr>), Vec<&'a Component>>,
    /// (network, role) -> the component providing it
    roles: BTreeMap<(String, String), &'a str>,
    netbios: BTreeMap<String, &'a str>,
    host_bound_netbios: usize,
}

impl<'a> Claims<'a> {
    /// take the ports, address, roles and NetBIOS of one attachment; report what was taken.
    fn claim(
        &mut self,
        site: &Site<'a>,
        network: &Network,
        address: Option<Ipv4Addr>,
        at: &str,
    ) -> Vec<ValidationError> {
        let mut errors = self.claim_ports(site, network, address, at);
        self.addresses
            .entry((network.id.clone(), address))
            .or_default()
            .push(site.component);
        errors.extend(self.claim_roles(site, network, at));
        errors.extend(self.claim_netbios(site, network, at));
        errors
    }

    fn claim_ports(
        &mut self,
        site: &Site<'a>,
        network: &Network,
        address: Option<Ipv4Addr>,
        at: &str,
    ) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        for &(proto, port) in site.component.ports {
            let key = (network.id.clone(), address, proto, port);
            if let Some(other) = self.ports.insert(key, site.id) {
                errors.push(fail(
                    format!("{at}.address"),
                    format!(
                        "{} and {other} both want {} on the same address of {}",
                        site.component.name(),
                        Component::port_text(proto, port),
                        network.name
                    ),
                ));
            }
        }
        errors
    }

    fn claim_roles(
        &mut self,
        site: &Site<'a>,
        network: &Network,
        at: &str,
    ) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        for role in site.component.roles {
            let key = (network.id.clone(), format!("{role:?}"));
            if let Some(other) = self.roles.insert(key, site.id) {
                errors.push(fail(
                    format!("{at}.network"),
                    format!(
                        "{} and {other} both provide {role:?} on {}",
                        site.component.name(),
                        network.name
                    ),
                ));
            }
        }
        errors
    }

    fn claim_netbios(
        &mut self,
        site: &Site<'a>,
        network: &Network,
        at: &str,
    ) -> Option<ValidationError> {
        if !site.component.netbios {
            return None;
        }
        let other = self.netbios.insert(network.id.clone(), site.id)?;
        Some(fail(
            format!("{at}.network"),
            format!(
                "{} and {other} both speak NetBIOS on {}",
                site.component.name(),
                network.name
            ),
        ))
    }

    /// a component that must have its address to itself but shares it.
    fn shared_address_errors(&self) -> Vec<ValidationError> {
        self.addresses
            .iter()
            .filter(|(_, on)| on.len() > 1)
            .filter_map(|((network, address), on)| {
                let lone = on.iter().find(|c| c.needs_own_address)?;
                let address = (*address)?;
                Some(fail(
                    format!("{SECTION}.{}.attach", lone.id()),
                    format!("{} must have {address} on {network} to itself", lone.name()),
                ))
            })
            .collect()
    }

    fn netbios_host_error(&self) -> Option<ValidationError> {
        (self.host_bound_netbios > 1).then(|| {
            fail(
                SECTION.into(),
                "only one NetBIOS component can bind the gateway itself; give the others ipvlan"
                    .into(),
            )
        })
    }
}

fn fail(path: String, message: String) -> ValidationError {
    ValidationError { path, message }
}

/// too few or too many attachments for the component.
fn attach_count_errors(site: &Site) -> Vec<ValidationError> {
    let (component, path) = (site.component, &site.path);
    let count = site.choice.attach.len();
    if component.attach_max == Some(0) && count > 0 {
        return vec![fail(
            format!("{path}.attach"),
            format!(
                "{} runs on the gateway itself and joins no network",
                component.name()
            ),
        )];
    }
    let few = (count < component.attach_min).then(|| {
        fail(
            format!("{path}.attach"),
            format!(
                "{} needs at least {} network{}",
                component.name(),
                component.attach_min,
                if component.attach_min == 1 { "" } else { "s" }
            ),
        )
    });
    let many = component.attach_max.filter(|max| count > *max).map(|max| {
        fail(
            format!("{path}.attach"),
            format!("{} joins at most {max} networks", component.name()),
        )
    });
    few.into_iter().chain(many).collect()
}

/// everything wrong with one attachment, claiming what it takes as it goes.
fn attachment_errors<'a>(
    site: &Site<'a>,
    networks: &[Network],
    (j, attachment, earlier): (usize, &Attachment, &[Attachment]),
    claims: &mut Claims<'a>,
) -> Vec<ValidationError> {
    let at = format!("{}.attach[{j}]", site.path);
    let Some(network) = networks.iter().find(|n| n.id == attachment.network) else {
        return vec![fail(
            format!("{at}.network"),
            format!("there is no network called {}", attachment.network),
        )];
    };
    if let Some(error) = network_error(site, attachment, network, earlier, &at) {
        return vec![error];
    }
    let mut errors = Vec::new();
    if let Some(error) = address_error(site, attachment, network, &at) {
        errors.push(error);
        if attachment.address.is_some() {
            return errors;
        }
    }
    errors.extend(claims.claim(site, network, attachment.address, &at));
    errors
}

/// a network the component cannot join, or joins twice.
fn network_error(
    site: &Site,
    attachment: &Attachment,
    network: &Network,
    earlier: &[Attachment],
    at: &str,
) -> Option<ValidationError> {
    let component = site.component;
    let message = if earlier.iter().any(|a| a.network == attachment.network) {
        format!("{} joins {} twice", component.name(), network.name)
    } else if network.external && !component.external {
        format!("{} only joins LANlord networks", component.name())
    } else {
        return None;
    };
    Some(fail(format!("{at}.network"), message))
}

/// an address that does not fit, or a missing one the component needs.
fn address_error(
    site: &Site,
    attachment: &Attachment,
    network: &Network,
    at: &str,
) -> Option<ValidationError> {
    let (component, mechanism) = (site.component, site.choice.mechanism);
    let message = match attachment.address {
        Some(_) if network.external => {
            "only the gateway's address can be used on the outside network".to_string()
        }
        Some(address) => network.address_problem(address)?,
        None if mechanism != Mechanism::HostBind => format!(
            "{} runs as {:?} and needs an address of its own on {}",
            component.name(),
            mechanism,
            network.name
        ),
        None if component.needs_own_address => format!(
            "{} needs an address of its own on {}",
            component.name(),
            network.name
        ),
        None => return None,
    };
    Some(fail(format!("{at}.address"), message))
}

impl Component {
    fn port_text(proto: Proto, port: u16) -> String {
        let proto = match proto {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        };
        format!("{proto} {port}")
    }
}

/// an address equal to the gateway's means "the gateway's address".
fn normalise(choice: &mut Choice, networks: &[Network]) {
    for attachment in &mut choice.attach {
        let gateway = networks
            .iter()
            .find(|n| n.id == attachment.network)
            .and_then(|n| n.gateway);
        if attachment.address.is_some() && attachment.address == gateway {
            attachment.address = None;
        }
    }
}

/// the id of the DHCP server entry, which used to be a subcomponent of `dns`.
const DHCP: &str = "dhcp";

/// settings from before the DHCP server became an entry of its own have none: it is on next to
/// the DNS server, joined to the same networks, unless the old `dns` subcomponents left it out
/// (or left the DNS server out, which then goes).
fn split_dhcp(enabled: &mut BTreeMap<String, Choice>) {
    let Some(dns) = enabled.get_mut("dns") else {
        return;
    };
    let parts = std::mem::take(&mut dns.subcomponents);
    let has = |id: &str| parts.is_empty() || parts.iter().any(|p| p == id);
    let (dns_on, dhcp_on) = (has("dns"), has(DHCP));
    if dhcp_on {
        let choice = Choice {
            attach: dns.attach.clone(),
            mechanism: dns.mechanism,
            subcomponents: Vec::new(),
        };
        enabled.insert(DHCP.into(), choice);
    }
    if !dns_on {
        enabled.remove("dns");
    }
}

/// the old way: every module with a top-level section is on, joined to the one internal network;
/// `nt4-domain.address` and `web-proxy.listen` become attachments.
fn legacy(settings: &Settings, networks: &[Network]) -> BTreeMap<String, Choice> {
    let lan = networks
        .iter()
        .find(|n| !n.external)
        .map_or_else(|| LEGACY_NETWORK.to_string(), |n| n.id.clone());
    let external = networks
        .iter()
        .find(|n| n.external)
        .map_or_else(|| EXTERNAL_NETWORK.to_string(), |n| n.id.clone());
    catalogue()
        .iter()
        .filter(|c| settings.section(c.id()).is_some())
        .map(|c| {
            let section = settings.section(c.id());
            let mut attach = Vec::new();
            if c.id() == "nt4-domain" {
                // the address is on the first nt4 server once the settings are migrated
                let server = match settings.section("nt4-servers") {
                    Some(Value::Sequence(list)) => list.first(),
                    _ => None,
                };
                let address = section
                    .and_then(|s| s.get("address"))
                    .or_else(|| server.and_then(|s| s.get("address")))
                    .and_then(Value::as_str)
                    .and_then(cidr)
                    .map(|(address, _)| address);
                attach.push(Attachment {
                    network: lan.clone(),
                    address,
                });
            } else if c.id() == "web-proxy" {
                attach = listen(
                    section.and_then(|s| s.get("listen")),
                    &lan,
                    &external,
                    networks,
                );
            } else if c.attach_min > 0 {
                attach.push(Attachment {
                    network: lan.clone(),
                    address: None,
                });
            }
            (
                c.id().to_string(),
                Choice {
                    attach,
                    mechanism: Mechanism::default(),
                    subcomponents: c.default_subcomponents(),
                },
            )
        })
        .collect()
}

/// `web-proxy.listen` (default `[host, internal]`) as attachments: `host` is the outside
/// network, `internal` the internal one, an address joins the network it is in.
fn listen(
    value: Option<&Value>,
    lan: &str,
    external: &str,
    networks: &[Network],
) -> Vec<Attachment> {
    let items: Vec<String> = match value {
        None | Some(Value::Null) => vec!["host".into(), "internal".into()],
        Some(Value::Sequence(list)) => list
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(Value::String(one)) => vec![one.clone()],
        Some(_) => Vec::new(),
    };
    let mut attach: Vec<Attachment> = Vec::new();
    for item in items {
        let attachment = match item.trim() {
            "host" => Attachment {
                network: external.into(),
                address: None,
            },
            "internal" => Attachment {
                network: lan.into(),
                address: None,
            },
            other => {
                let Some(address) = other
                    .split('/')
                    .next()
                    .and_then(|a| a.parse::<Ipv4Addr>().ok())
                else {
                    continue;
                };
                // an address in a known network joins it there; any other one was an address of
                // the gateway's own outside interface
                match networks
                    .iter()
                    .find(|n| !n.external && n.subnet.is_some_and(|s| s.contains(address)))
                {
                    Some(network) => Attachment {
                        network: network.id.clone(),
                        address: Some(address),
                    },
                    None => Attachment {
                        network: external.into(),
                        address: None,
                    },
                }
            }
        };
        if !attach.iter().any(|a| a.network == attachment.network) {
            attach.push(attachment);
        }
    }
    attach
}

/// a network as the components see it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    /// the gateway's normal (outside) network rather than a LANlord network.
    pub external: bool,
    /// unknown for an outside network the settings do not describe.
    pub subnet: Option<Subnet>,
    /// the gateway's own address there.
    pub gateway: Option<Ipv4Addr>,
    /// the first and last address DHCP hands out, when it does.
    pub dhcp: Option<(Ipv4Addr, Ipv4Addr)>,
    /// the addresses reserved for machines by their mac address.
    pub reservations: Vec<Ipv4Addr>,
}

impl Network {
    /// why `address` cannot be a component's own address here, if it cannot.
    pub fn address_problem(&self, address: Ipv4Addr) -> Option<String> {
        let Some(subnet) = self.subnet else {
            return Some(format!("{} has no subnet in the settings", self.name));
        };
        if !subnet.contains(address) {
            Some(format!("{address} is outside {subnet}"))
        } else if subnet.prefix < 31 && (address == subnet.network || address == subnet.broadcast())
        {
            Some(format!(
                "{address} is the network or broadcast address of {subnet}"
            ))
        } else if let Some((start, end)) = self
            .dhcp
            .filter(|(start, end)| (*start..=*end).contains(&address))
        {
            Some(format!("{address} is inside the DHCP range {start}-{end}"))
        } else if self.reservations.contains(&address) {
            Some(format!("{address} is reserved for a machine"))
        } else {
            None
        }
    }
}

/// the host's networks: the gateway's outside network first (always there, named `external`
/// unless the `networks` list has a `kind: external` entry), then the LANlord networks of the
/// `networks` list, or else the one internal network older settings describe with
/// `docker.internal_address`, `dns.dhcp.range` and `dns.dhcp.hosts`.
pub fn networks_from_settings(settings: &Settings) -> Vec<Network> {
    let mut networks = Vec::new();
    if let Some(Value::Sequence(list)) = settings.section("networks") {
        networks.extend(list.iter().filter_map(listed_network));
    } else if let Some(gateway) = settings
        .section("docker")
        .and_then(|d| d.get("internal_address"))
        .and_then(Value::as_str)
        .and_then(cidr)
    {
        let dhcp = settings.section("dns").and_then(|d| d.get("dhcp"));
        networks.push(Network {
            id: LEGACY_NETWORK.into(),
            name: "Internal network".into(),
            external: false,
            subnet: Some(Subnet::new(gateway.0, gateway.1)),
            gateway: Some(gateway.0),
            dhcp: dhcp.and_then(range),
            reservations: dhcp
                .map(|d| reservations(d, "hosts", "address"))
                .unwrap_or_default(),
        });
    }
    if !networks.iter().any(|n| n.external) {
        networks.insert(
            0,
            Network {
                id: EXTERNAL_NETWORK.into(),
                name: "External network".into(),
                external: true,
                subnet: None,
                gateway: None,
                dhcp: None,
                reservations: Vec::new(),
            },
        );
    }
    networks
}

fn listed_network(entry: &Value) -> Option<Network> {
    // the Networks window writes `name`, `interface` and `address` (the gateway's address with
    // its prefix); a hand-written entry may carry `id`, `subnet` and `gateway` instead
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    let id = text("id").or_else(|| text("name"))?.to_string();
    let external = text("kind") == Some("external");
    let own = text("address").and_then(cidr);
    let subnet = text("subnet")
        .and_then(cidr)
        .or(own)
        .map(|(network, prefix)| Subnet::new(network, prefix));
    let gateway = text("gateway")
        .and_then(|g| g.parse().ok())
        .or_else(|| own.map(|(address, _)| address));
    if !external && (subnet.is_none() || gateway.is_none()) {
        return None;
    }
    let dhcp = entry.get("dhcp");
    Some(Network {
        name: text("name").map_or_else(|| id.clone(), String::from),
        id,
        external,
        subnet,
        gateway,
        dhcp: dhcp.and_then(range),
        reservations: dhcp
            .map(|d| {
                let mut found = reservations(d, "reservations", "ip");
                found.extend(reservations(d, "hosts", "address"));
                found
            })
            .unwrap_or_default(),
    })
}

/// the addresses in a list of reservations under a dhcp mapping.
fn reservations(dhcp: &Value, list: &str, key: &str) -> Vec<Ipv4Addr> {
    let Some(Value::Sequence(entries)) = dhcp.get(list) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| e.get(key).and_then(Value::as_str)?.parse().ok())
        .collect()
}

/// `a.b.c.d/p` into the address and the prefix.
fn cidr(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (address, prefix) = text.trim().split_once('/')?;
    Some((address.parse().ok()?, prefix.parse().ok()?))
}

/// a `range: [first, last]` under a dhcp mapping.
fn range(dhcp: &Value) -> Option<(Ipv4Addr, Ipv4Addr)> {
    let Value::Sequence(range) = dhcp.get("range")? else {
        return None;
    };
    let text = |i: usize| -> Option<Ipv4Addr> { range.get(i)?.as_str()?.parse().ok() };
    Some((text(0)?, text(1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan(id: &str, third: u8) -> Network {
        Network {
            id: id.into(),
            name: id.into(),
            external: false,
            subnet: Some(Subnet::new(Ipv4Addr::new(192, 0, third, 0), 24)),
            gateway: Some(Ipv4Addr::new(192, 0, third, 1)),
            dhcp: Some((
                Ipv4Addr::new(192, 0, third, 100),
                Ipv4Addr::new(192, 0, third, 200),
            )),
            reservations: vec![Ipv4Addr::new(192, 0, third, 20)],
        }
    }

    fn nets() -> Vec<Network> {
        let mut nets = vec![lan("retronet", 2), lan("lab", 3)];
        nets.insert(
            0,
            Network {
                id: EXTERNAL_NETWORK.into(),
                name: "outside".into(),
                external: true,
                subnet: None,
                gateway: None,
                dhcp: None,
                reservations: Vec::new(),
            },
        );
        nets
    }

    fn at(network: &str, address: Option<[u8; 4]>) -> Attachment {
        Attachment {
            network: network.into(),
            address: address.map(Ipv4Addr::from),
        }
    }

    fn with(entries: &[(&str, Vec<Attachment>)]) -> Selection {
        let mut enabled: BTreeMap<String, Choice> = entries
            .iter()
            .map(|(id, attach)| {
                (
                    id.to_string(),
                    Choice {
                        attach: attach.clone(),
                        ..Choice::default()
                    },
                )
            })
            .collect();
        enabled.entry("gateway".into()).or_default();
        enabled.entry("docker".into()).or_default();
        Selection { enabled }
    }

    fn messages(selection: &Selection) -> Vec<String> {
        selection
            .validate(&nets())
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    #[test]
    fn the_catalogue_is_the_module_list() {
        let catalogue = catalogue();
        assert_eq!(catalogue.len(), MODULES.len());
        for (component, module) in catalogue.iter().zip(MODULES) {
            assert_eq!(component.id(), module.id);
            assert_eq!(component.name(), module.name);
            assert_eq!(component.mechanisms[0], Mechanism::HostBind);
        }
        assert!(find("gateway").unwrap().required);
        assert!(!find("docker").unwrap().attaches());
        assert!(find("nt4-domain").unwrap().netbios);
        assert!(find("web-proxy").unwrap().external);
        assert_eq!(find("dns").unwrap().ports(), "udp 53, tcp 53");
        assert_eq!(find("dhcp").unwrap().ports(), "udp 67");
        assert_eq!(find("dns").unwrap().needs(), "Docker, 1+ network");
        assert_eq!(find("dhcp").unwrap().needs(), "Docker, 1+ network");
        assert!(find("dns").unwrap().subcomponents.is_empty());
        assert_eq!(find("gateway").unwrap().needs(), "gateway only");
        let time = find("time-server").unwrap();
        assert_eq!(time.name(), "Time Server (NTP)");
        assert_eq!(time.ports(), "udp 123");
        assert_eq!(time.needs(), "Docker, 1+ network");
        assert!(time.roles.contains(&Role::Ntp) && !time.external);
        assert_eq!(
            find("remote-commands").unwrap().needs(),
            "networks optional"
        );
    }

    #[test]
    fn a_component_may_join_several_networks_and_share_them() {
        let selection = with(&[
            (
                "nt4-domain",
                vec![at("retronet", Some([192, 0, 2, 5])), at("lab", None)],
            ),
            ("dns", vec![at("retronet", None), at("lab", None)]),
            (
                "web-proxy",
                vec![
                    at("retronet", Some([192, 0, 2, 5])),
                    at(EXTERNAL_NETWORK, None),
                ],
            ),
        ]);
        assert!(
            messages(&selection).is_empty(),
            "{:?}",
            messages(&selection)
        );
    }

    #[test]
    fn addresses_must_fit_the_network() {
        for (address, says) in [
            ([198, 51, 100, 5], "outside"),
            ([192, 0, 2, 255], "broadcast"),
            ([192, 0, 2, 150], "DHCP range"),
            ([192, 0, 2, 20], "reserved"),
        ] {
            let selection = with(&[("nt4-domain", vec![at("retronet", Some(address))])]);
            let got = messages(&selection);
            assert!(
                got.len() == 1 && got[0].contains(says),
                "{address:?}: {got:?}"
            );
        }
    }

    #[test]
    fn components_with_different_ports_share_an_address() {
        let mut selection = with(&[
            ("web-proxy", vec![at("retronet", None)]),
            ("nt4-domain", vec![at("retronet", None)]),
        ]);
        assert!(
            messages(&selection).is_empty(),
            "different ports may share an address"
        );
        selection.enabled.get_mut("nt4-domain").unwrap().attach =
            vec![at("retronet", Some([192, 0, 2, 5]))];
        assert!(messages(&selection).is_empty());
    }

    #[test]
    fn components_with_different_roles_share_a_network() {
        // clashing roles, ports or netbios need two instances of one kind, which come later
        let mut selection = with(&[("nt4-domain", vec![at("retronet", None)])]);
        selection.enabled.insert(
            "dns".into(),
            Choice {
                attach: vec![at("retronet", None)],
                ..Choice::default()
            },
        );
        assert!(messages(&selection).is_empty());
    }

    #[test]
    fn the_other_rules() {
        let cases: Vec<(Selection, &str)> = vec![
            (with(&[("nope", vec![])]), "no component called nope"),
            (with(&[("dns", vec![])]), "needs at least 1 network"),
            (
                with(&[("dns", vec![at("moon", None)])]),
                "no network called moon",
            ),
            (
                with(&[("dns", vec![at("lab", None), at("lab", None)])]),
                "joins lab twice",
            ),
            (
                with(&[("docker", vec![at("lab", None)])]),
                "joins no network",
            ),
            (
                with(&[("dns", vec![at(EXTERNAL_NETWORK, None)])]),
                "only joins LANlord networks",
            ),
            (
                with(&[(
                    "web-proxy",
                    vec![at(EXTERNAL_NETWORK, Some([192, 0, 2, 9]))],
                )]),
                "only the gateway's address",
            ),
        ];
        for (selection, says) in cases {
            let got = messages(&selection);
            assert!(got.iter().any(|m| m.contains(says)), "{says}: {got:?}");
        }
        let mut no_docker = with(&[("dns", vec![at("lab", None)])]);
        no_docker.enabled.remove("docker");
        assert!(messages(&no_docker)
            .iter()
            .any(|m| m.contains("needs Docker")));
        let mut no_gateway = with(&[]);
        no_gateway.enabled.remove("gateway");
        assert!(messages(&no_gateway)
            .iter()
            .any(|m| m.contains("cannot be removed")));
        let mut parts = with(&[("dns", vec![at("lab", None)])]);
        parts.enabled.get_mut("dns").unwrap().subcomponents = vec!["teleport".into()];
        assert!(messages(&parts)
            .iter()
            .any(|m| m.contains("no part called")));
        let mut mac = with(&[("dns", vec![at("lab", None)])]);
        mac.enabled.get_mut("dns").unwrap().mechanism = Mechanism::Macvlan;
        assert!(messages(&mac).iter().any(|m| m.contains("cannot run as")));
    }

    #[test]
    fn networks_written_by_the_networks_window_are_listed() {
        let settings = Settings::parse(
            "networks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n    dhcp:\n      range: [172.16.120.100, 172.16.120.200]\n      hosts:\n        - {name: ws1, mac: '02:00:00:00:00:01', address: 172.16.120.20}\n",
        )
        .unwrap();
        let networks = networks_from_settings(&settings);
        let ids: Vec<&str> = networks.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, [EXTERNAL_NETWORK, "retronet"]);
        let lan = &networks[1];
        assert_eq!(lan.name, "retronet");
        assert_eq!(lan.subnet.unwrap().to_string(), "172.16.120.0/22");
        assert_eq!(lan.gateway, Some(Ipv4Addr::new(172, 16, 120, 1)));
        assert_eq!(
            lan.dhcp,
            Some((
                Ipv4Addr::new(172, 16, 120, 100),
                Ipv4Addr::new(172, 16, 120, 200)
            ))
        );
        assert_eq!(lan.reservations, [Ipv4Addr::new(172, 16, 120, 20)]);
    }

    #[test]
    fn old_settings_switch_on_the_modules_they_configure() {
        let settings = Settings::parse(
            "docker:\n  internal_address: 192.0.2.1/24\ndns:\n  zone: retronet\n  dhcp:\n    range: [192.0.2.100, 192.0.2.200]\n    hosts:\n      - {name: ws1, mac: '02:00:00:00:00:01', address: 192.0.2.20}\nnt4-domain:\n  address: 192.0.2.5/24\nweb-proxy:\n  listen: [host, 192.0.2.6]\n",
        )
        .unwrap();
        let networks = networks_from_settings(&settings);
        let ids: Vec<&str> = networks.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, [EXTERNAL_NETWORK, LEGACY_NETWORK]);
        let internal = &networks[1];
        assert_eq!(internal.gateway, Some(Ipv4Addr::new(192, 0, 2, 1)));
        assert_eq!(internal.subnet.unwrap().to_string(), "192.0.2.0/24");
        assert_eq!(internal.reservations, [Ipv4Addr::new(192, 0, 2, 20)]);
        let selection = Selection::from_settings(&settings).unwrap();
        let ids: Vec<&str> = selection.enabled.keys().map(String::as_str).collect();
        assert_eq!(
            ids,
            [
                "dhcp",
                "dns",
                "docker",
                "gateway",
                "nt4-domain",
                "web-proxy"
            ]
        );
        for id in ["dns", "dhcp"] {
            assert_eq!(selection.enabled[id].attach, vec![at(LEGACY_NETWORK, None)]);
            assert!(selection.enabled[id].subcomponents.is_empty());
        }
        assert_eq!(
            selection.enabled["nt4-domain"].attach,
            vec![at(LEGACY_NETWORK, Some([192, 0, 2, 5]))]
        );
        assert_eq!(
            selection.enabled["web-proxy"].attach,
            vec![
                at(EXTERNAL_NETWORK, None),
                at(LEGACY_NETWORK, Some([192, 0, 2, 6]))
            ]
        );
        assert!(selection.enabled["docker"].attach.is_empty());
        assert!(
            selection.validate(&networks).is_empty(),
            "{:?}",
            selection.validate(&networks)
        );
    }

    #[test]
    fn the_time_server_joins_lan_networks_beside_network_services() {
        // it shares a gateway address with dns and dhcp: udp 123 clashes with neither
        let both = with(&[
            ("dns", vec![at("retronet", None), at("lab", None)]),
            ("time-server", vec![at("retronet", None), at("lab", None)]),
        ]);
        assert!(messages(&both).is_empty(), "{:?}", messages(&both));
        // two time servers on one network would both provide Ntp and want udp 123
        let mut selection = with(&[("time-server", vec![at("retronet", None)])]);
        assert!(messages(&selection).is_empty());
        selection
            .enabled
            .get_mut("time-server")
            .unwrap()
            .attach
            .push(at("retronet", None));
        assert!(messages(&selection).iter().any(|m| m.contains("twice")));
        // it needs a network, Docker and a LANlord (not the outside) network
        for (selection, want) in [
            (with(&[("time-server", vec![])]), "needs at least 1 network"),
            (
                with(&[("time-server", vec![at(EXTERNAL_NETWORK, None)])]),
                "only joins LANlord networks",
            ),
        ] {
            let found = messages(&selection);
            assert!(found.iter().any(|m| m.contains(want)), "{want}: {found:?}");
        }
        let mut no_docker = with(&[("time-server", vec![at("lab", None)])]);
        no_docker.enabled.remove("docker");
        assert!(messages(&no_docker)
            .iter()
            .any(|m| m.contains("Time Server (NTP) needs Docker")));
    }

    #[test]
    fn a_time_server_section_or_entry_reads_back_with_its_networks() {
        // an old-style section alone switches it on for the one internal network
        let old = Settings::parse(
            "docker:\n  internal_address: 192.0.2.1/24\ntime-server:\n  pools: [pool.ntp.org]\n  servers: [192.0.2.123]\n",
        )
        .unwrap();
        let selection = Selection::from_settings(&old).unwrap();
        assert_eq!(
            selection.enabled["time-server"].attach,
            vec![at(LEGACY_NETWORK, None)]
        );
        // a components entry names the networks, and enabled: false switches it off
        let listed = Settings::parse(
            "networks:\n  - {id: retronet, subnet: 192.0.2.0/24, gateway: 192.0.2.1}\n  - {id: lab, subnet: 198.51.100.0/24, gateway: 198.51.100.1}\ncomponents:\n  gateway: {}\n  docker: {}\n  time-server:\n    attach: [{network: lab}]\ntime-server:\n  pools: [pool.ntp.org]\n",
        )
        .unwrap();
        let selection = Selection::from_settings(&listed).unwrap();
        assert_eq!(
            selection.enabled["time-server"].attach,
            vec![at("lab", None)]
        );
        let off = Settings::parse("components:\n  gateway: {}\n  time-server: {enabled: false}\n")
            .unwrap();
        assert!(!Selection::from_settings(&off)
            .unwrap()
            .is_enabled("time-server"));
    }

    #[test]
    fn the_gateways_own_address_means_no_address() {
        let settings = Settings::parse(
            "docker:\n  internal_address: 192.0.2.1/24\nnt4-domain:\n  address: 192.0.2.1/24\n",
        )
        .unwrap();
        let selection = Selection::from_settings(&settings).unwrap();
        assert_eq!(
            selection.enabled["nt4-domain"].attach,
            vec![at(LEGACY_NETWORK, None)]
        );
        // web-proxy's default listen is both networks
        let settings =
            Settings::parse("docker:\n  internal_address: 192.0.2.1/24\nweb-proxy: {}\n").unwrap();
        let selection = Selection::from_settings(&settings).unwrap();
        assert_eq!(
            selection.enabled["web-proxy"].attach,
            vec![at(EXTERNAL_NETWORK, None), at(LEGACY_NETWORK, None)]
        );
    }

    #[test]
    fn nothing_configured_still_has_the_gateway() {
        let selection = Selection::from_settings(&Settings::empty()).unwrap();
        assert_eq!(selection.enabled.keys().collect::<Vec<_>>(), ["gateway"]);
        let networks = networks_from_settings(&Settings::empty());
        assert_eq!(networks.len(), 1);
        assert!(networks[0].external);
    }

    #[test]
    fn the_choice_round_trips_and_keeps_other_keys() {
        let mut settings = Settings::parse(
            "docker: {}\nnt4-domain:\n  mode: domain\ncomponents:\n  nt4-domain:\n    name: RETRODOM\n  web-proxy:\n    sites: []\n",
        )
        .unwrap();
        let mut selection = with(&[
            (
                "nt4-domain",
                vec![at("retronet", Some([192, 0, 2, 5])), at("lab", None)],
            ),
            ("dns", vec![at("retronet", None)]),
        ]);
        selection.enabled.get_mut("dns").unwrap().mechanism = Mechanism::Ipvlan;
        selection.apply_to(&mut settings);
        let text = settings.text();
        assert!(text.contains("address: 192.0.2.5"), "{text}");
        assert!(text.contains("name: RETRODOM"), "{text}");
        assert!(text.contains("mechanism: ipvlan"), "{text}");
        // switched off but kept, with its own keys
        let proxy = settings.section(SECTION).unwrap().get("web-proxy").unwrap();
        assert_eq!(proxy.get("enabled"), Some(&Value::Bool(false)));
        assert!(proxy.get("sites").is_some());
        // the old top-level sections stay for the playbooks that read them
        assert_eq!(
            settings.section("nt4-domain").unwrap().get("mode").unwrap(),
            "domain"
        );
        let back = Selection::from_settings(&Settings::parse(&text).unwrap()).unwrap();
        assert_eq!(back, selection);
    }

    #[test]
    fn a_networks_list_describes_the_networks() {
        let settings = Settings::parse(
            "networks:\n  - {id: wan, kind: external, name: Uplink}\n  - id: retronet\n    name: Retro LAN\n    subnet: 192.0.2.0/24\n    gateway: 192.0.2.1\n    dhcp: {range: [192.0.2.100, 192.0.2.200], reservations: [{mac: '02:00:00:00:00:10', ip: 192.0.2.20}]}\n  - {id: lab, subnet: 198.51.100.0/24, gateway: 198.51.100.1}\n  - {id: broken}\n",
        )
        .unwrap();
        let networks = networks_from_settings(&settings);
        let ids: Vec<&str> = networks.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["wan", "retronet", "lab"]);
        assert!(networks[0].external);
        assert_eq!(networks[1].name, "Retro LAN");
        assert_eq!(networks[1].reservations, [Ipv4Addr::new(192, 0, 2, 20)]);
        assert_eq!(networks[2].dhcp, None);
        // older settings with a list attach to its first lan, and `host` to its outside network
        let legacy = Settings::parse(&format!("{}web-proxy: {{}}\n", settings.text())).unwrap();
        let selection = Selection::from_settings(&legacy).unwrap();
        assert_eq!(
            selection.enabled["web-proxy"].attach,
            vec![at("wan", None), at("retronet", None)]
        );
    }

    #[test]
    fn clearing_a_component_on_through_its_old_section_keeps_it_off() {
        let mut settings = Settings::parse("docker: {}\ndns: {}\n").unwrap();
        let mut selection = Selection::from_settings(&settings).unwrap();
        assert!(selection.is_enabled("dns"));
        selection.enabled.remove("dns");
        selection.apply_to(&mut settings);
        let back = Selection::from_settings(&Settings::parse(&settings.text()).unwrap()).unwrap();
        assert!(!back.is_enabled("dns"), "{}", settings.text());
        assert!(back.is_enabled("docker"));
        assert!(settings.section("dns").is_some(), "the old section stays");
    }

    #[test]
    fn a_listen_address_outside_every_lan_is_the_gateways_outside_address() {
        let settings = Settings::parse(
            "docker:\n  internal_address: 192.0.2.1/24\nweb-proxy:\n  listen: [203.0.113.7, internal]\n",
        )
        .unwrap();
        let networks = networks_from_settings(&settings);
        let selection = Selection::from_settings(&settings).unwrap();
        assert_eq!(
            selection.enabled["web-proxy"].attach,
            vec![at(EXTERNAL_NETWORK, None), at(LEGACY_NETWORK, None)]
        );
        assert!(selection.validate(&networks).is_empty());
    }

    #[test]
    fn a_namespaced_component_needs_its_own_address() {
        let mut selection = with(&[("dns", vec![at("lab", None)])]);
        selection.enabled.get_mut("dns").unwrap().mechanism = Mechanism::Ipvlan;
        assert!(messages(&selection)
            .iter()
            .any(|m| m.contains("needs an address of its own")));
        selection.enabled.get_mut("dns").unwrap().attach = vec![at("lab", Some([192, 0, 3, 9]))];
        assert!(
            messages(&selection).is_empty(),
            "{:?}",
            messages(&selection)
        );
    }

    #[test]
    fn the_old_sections_still_read_when_the_new_one_is_broken() {
        let settings = Settings::parse("docker: {}\ndns: {}\ncomponents: 7\n").unwrap();
        assert!(Selection::from_settings(&settings).is_err());
        let fallback = Selection::from_sections(&settings);
        assert!(fallback.is_enabled("dns") && fallback.is_enabled("gateway"));
    }

    #[test]
    fn enabled_false_switches_off_and_bad_shapes_are_errors() {
        let settings = Settings::parse("dns: {}\ncomponents:\n  dns: {enabled: false}\n").unwrap();
        assert!(!Selection::from_settings(&settings)
            .unwrap()
            .is_enabled("dns"));
        let empty = Settings::parse("components:\n  dns:\n").unwrap();
        assert!(Selection::from_settings(&empty).unwrap().is_enabled("dns"));
        assert!(
            Selection::from_settings(&Settings::parse("components: [1, 2]\n").unwrap()).is_err()
        );
        assert!(Selection::from_settings(
            &Settings::parse("components: {dns: {attach: 3}}\n").unwrap()
        )
        .is_err());
    }

    fn read(text: &str) -> Selection {
        Selection::from_settings(&Settings::parse(text).unwrap()).unwrap()
    }

    #[test]
    fn settings_from_before_the_split_come_up_with_both_ticked() {
        let both = read("components:\n  dns: {attach: [{network: lab}]}\n");
        assert_eq!(both.enabled["dns"].attach, both.enabled["dhcp"].attach);
        let listed = read("components:\n  dns: {subcomponents: [dns, dhcp]}\n");
        assert!(listed.is_enabled("dns") && listed.is_enabled("dhcp"));
        assert!(listed.enabled["dns"].subcomponents.is_empty());
        // an old choice that left one out keeps leaving it out
        let only_dns = read("components:\n  dns: {subcomponents: [dns]}\n");
        assert!(only_dns.is_enabled("dns") && !only_dns.is_enabled("dhcp"));
        let only_dhcp = read("components:\n  dns: {subcomponents: [dhcp]}\n");
        assert!(!only_dhcp.is_enabled("dns") && only_dhcp.is_enabled("dhcp"));
        let off = read("components:\n  dns: {enabled: false}\n");
        assert!(!off.is_enabled("dns") && !off.is_enabled("dhcp"));
    }

    #[test]
    fn a_dhcp_entry_is_taken_as_written() {
        let text = "components:\n  dns: {}\n  dhcp: {enabled: false}\n";
        let s = read(text);
        assert!(s.is_enabled("dns") && !s.is_enabled("dhcp"));
        let s = read("components:\n  dhcp: {attach: [{network: lab}]}\n");
        assert!(!s.is_enabled("dns") && s.is_enabled("dhcp"));
    }

    #[test]
    fn unticking_dhcp_is_written_so_it_stays_off() {
        let mut settings = Settings::parse("components:\n  dns: {}\n").unwrap();
        let mut s = read("components:\n  dns: {}\n");
        s.enabled.remove("dhcp");
        s.apply_to(&mut settings);
        let back = Selection::from_settings(&settings).unwrap();
        assert!(back.is_enabled("dns") && !back.is_enabled("dhcp"));
        // dhcp without dns is the third state
        let mut s = back;
        s.enabled.remove("dns");
        s.enabled.insert("dhcp".into(), Choice::default());
        s.apply_to(&mut settings);
        let back = Selection::from_settings(&settings).unwrap();
        assert!(!back.is_enabled("dns") && back.is_enabled("dhcp"));
    }

    #[test]
    fn only_dhcp_ticked_writes_dns_off_beside_it() {
        // a networks-list install: no dns section, only dhcp ticked
        let mut settings = Settings::parse(
            "networks:\n  - {name: lab, interface: eth1, address: 172.16.120.1/22}\n",
        )
        .unwrap();
        let mut s = Selection::default();
        s.enabled.insert("dhcp".into(), Choice::default());
        s.apply_to(&mut settings);
        let text = serde_yaml_ng::to_string(settings.section("components").unwrap()).unwrap();
        assert!(
            text.contains("dns:") && text.contains("enabled: false"),
            "{text}"
        );
        let back = Selection::from_settings(&settings).unwrap();
        assert!(!back.is_enabled("dns") && back.is_enabled("dhcp"));
    }
}
