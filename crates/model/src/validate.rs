//! strict rules for every value a client can send the server.
//!
//! settings end up as ansible variables (`-e @settings.yml`) in playbooks that run as root, and
//! the other operations name accounts, files and time zones. so the server checks each field
//! against an allow-list before anything is written or run, whatever the client already did; the
//! clients use the same functions for early feedback.
//!
//! the primitive rules (`hostname`, `ipv4_address`, `interface_name`, ...) return the reason a
//! value is refused. [`settings_document`] walks a whole settings document against the schema of
//! each section and names the field that is wrong, like `web-proxy.sites[0].host: ...`.
//! [`for_ansible`] is what the playbooks are given: the same checks, and every string marked
//! `!unsafe` so that ansible never templates it.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use serde_yaml_ng::value::{Tag, TaggedValue};
use serde_yaml_ng::{Mapping, Value};

use crate::access::Access;
use crate::modules::{self, MODULES};
use crate::operations::{Operation, UserChange};

/// why a value is refused.
pub type Check = Result<(), &'static str>;

/// the field that was refused and why, as one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub field: String,
    pub reason: String,
}

impl Invalid {
    /// control characters in the field or the reason are written escaped, so a hostile key
    /// cannot start a second line in a log.
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: escape_controls(&field.into()),
            reason: escape_controls(&reason.into()),
        }
    }
}

fn escape_controls(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_debug().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.reason)
    }
}

impl std::error::Error for Invalid {}

/// the longest text in a single-line field, and in a multi-line one.
pub const LINE_LIMIT: usize = 1024;
pub const BLOCK_LIMIT: usize = 16 * 1024;
/// how deep a settings document may nest, and how many values it may hold.
const DEPTH_LIMIT: usize = 12;
const NODE_LIMIT: usize = 20_000;

fn flag(ok: bool, reason: &'static str) -> Check {
    ok.then_some(()).ok_or(reason)
}

// ---- names and addresses ----

/// one rfc 1123 label: letters, digits and inner dashes, 1 to 63 characters.
pub fn label(text: &str) -> Check {
    let bytes = text.as_bytes();
    let edge = |byte: &u8| byte.is_ascii_alphanumeric();
    flag(
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes.first().is_some_and(edge)
            && bytes.last().is_some_and(edge)
            && bytes.iter().all(|byte| edge(byte) || *byte == b'-'),
        "must be letters, digits and inner dashes, 1 to 63 characters",
    )
}

fn dns_name(text: &str, least_labels: usize) -> Check {
    if text.len() > 253 {
        return Err("must be at most 253 characters");
    }
    let labels: Vec<&str> = text.split('.').collect();
    if labels.len() < least_labels {
        return Err("must be a fully qualified name like host.example.com");
    }
    for part in &labels {
        label(part).map_err(|_| "must be dot-separated labels of letters, digits and dashes")?;
    }
    let top = labels.last().copied().unwrap_or_default();
    flag(
        !top.bytes().all(|byte| byte.is_ascii_digit()),
        "must not end in a number: that would be an address",
    )
}

/// a host name as rfc 1123 has it: one label, or dot-separated labels, at most 253 characters.
pub fn hostname(text: &str) -> Check {
    dns_name(text, 1)
}

/// a dns domain or zone: the same rules, and a single label (`retronet`) is a domain too.
pub fn domain(text: &str) -> Check {
    dns_name(text, 1)
}

/// a fully qualified name: at least two labels.
pub fn fqdn(text: &str) -> Check {
    dns_name(text, 2)
}

pub fn ipv4_address(text: &str) -> Check {
    text.parse::<Ipv4Addr>()
        .map(|_| ())
        .map_err(|_| "must be an ipv4 address like 192.0.2.10")
}

pub fn ipv6_address(text: &str) -> Check {
    text.parse::<Ipv6Addr>()
        .map(|_| ())
        .map_err(|_| "must be an ipv6 address like 2001:db8::10")
}

pub fn ip_address(text: &str) -> Check {
    flag(
        ipv4_address(text).is_ok() || ipv6_address(text).is_ok(),
        "must be an ipv4 or ipv6 address",
    )
}

fn prefix_within(text: &str, most: u8) -> bool {
    !text.is_empty()
        && text.len() <= 3
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && text.parse::<u8>().is_ok_and(|prefix| prefix <= most)
}

/// an ipv4 address with a prefix length, like `192.0.2.1/24`.
pub fn ipv4_cidr(text: &str) -> Check {
    let valid = text.split_once('/').is_some_and(|(address, prefix)| {
        ipv4_address(address).is_ok() && prefix_within(prefix, 32)
    });
    flag(
        valid,
        "must be an ipv4 address with a prefix like 192.0.2.1/24",
    )
}

/// an ipv6 address with a prefix length, like `2001:db8::1/64`.
pub fn ipv6_cidr(text: &str) -> Check {
    let valid = text.split_once('/').is_some_and(|(address, prefix)| {
        ipv6_address(address).is_ok() && prefix_within(prefix, 128)
    });
    flag(
        valid,
        "must be an ipv6 address with a prefix like 2001:db8::1/64",
    )
}

pub fn cidr(text: &str) -> Check {
    flag(
        ipv4_cidr(text).is_ok() || ipv6_cidr(text).is_ok(),
        "must be an address with a prefix like 192.0.2.1/24 or 2001:db8::1/64",
    )
}

/// a mac address, `aa:bb:cc:dd:ee:ff` in either case.
pub fn mac_address(text: &str) -> Check {
    flag(
        crate::settings::valid_mac(text),
        "must look like aa:bb:cc:dd:ee:ff",
    )
}

/// a tcp or udp port from 1 to 65535.
pub fn port(number: i64) -> Check {
    flag(
        (1..=65535).contains(&number),
        "must be a port from 1 to 65535",
    )
}

pub fn port_text(text: &str) -> Check {
    let valid = text.len() <= 5
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && text.parse::<i64>().is_ok_and(|number| port(number).is_ok());
    flag(valid, "must be a port from 1 to 65535")
}

/// a network interface name as the kernel allows (`IFNAMSIZ` leaves 15 characters), narrowed to
/// letters, digits and `_ . -`, and never starting with a dash.
pub fn interface_name(text: &str) -> Check {
    let valid = !text.is_empty()
        && text.len() <= 15
        && text != "."
        && text != ".."
        && !text.starts_with('-')
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
    flag(
        valid,
        "must be a plain interface name like eth1 (letters, digits, _ . -, at most 15)",
    )
}

/// a time zone name like `Europe/Berlin`: parts of letters, digits and `_ + -` split by `/`.
/// the server also checks that its zone database has it (see `lanlord_core::validate`).
pub fn time_zone(text: &str) -> Check {
    let valid = !text.is_empty()
        && text.len() <= 64
        && text.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
        });
    flag(valid, "must be a zone name like Europe/Berlin")
}

/// an internet time server: a host name or an ip address.
pub fn time_server(text: &str) -> Check {
    flag(
        hostname(text).is_ok() || ip_address(text).is_ok(),
        "must be a host name or an ip address",
    )
}

/// a login name: lowercase, digits, `_` and `-`, at most 32, not starting with a digit or dash.
pub fn login_name(text: &str) -> Check {
    let mut bytes = text.bytes();
    let valid = text.len() <= 32
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first == b'_')
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        });
    flag(
        valid,
        "must be a login name: lowercase letters, digits, _ and -, at most 32",
    )
}

/// a name in the nt4 domain: a user, as the container accepts it.
pub fn domain_user(text: &str) -> Check {
    flag(
        text.len() <= 32 && crate::users::valid_name(text),
        "must start with a lowercase letter and hold lowercase letters, digits and _ . -",
    )
}

/// a netbios name or workgroup: up to 15 letters, digits, `_` and `-`.
pub fn netbios_name(text: &str) -> Check {
    flag(
        crate::nt4::netbios_ok(text),
        "must be up to 15 letters, digits, _ or -",
    )
}

/// a machine account name.
pub fn machine_name(text: &str) -> Check {
    flag(
        crate::nt4::valid_machine(text),
        "must be up to 15 letters, digits or -, starting with a letter",
    )
}

/// a dhcp lease time: 30m, 12h, 7d or infinite.
pub fn lease(text: &str) -> Check {
    flag(
        crate::settings::valid_lease(text),
        "must look like 30m, 12h, 7d or infinite",
    )
}

/// where the web ui is reached by the proxy: its own socket, or a `host:port`.
pub fn upstream(text: &str) -> Check {
    if let Some(name) = text
        .strip_prefix("unix//run/lanlord-web/")
        .and_then(|rest| rest.strip_suffix(".sock"))
    {
        return flag(
            !name.is_empty()
                && !name.starts_with('.')
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')),
            "must be unix//run/lanlord-web/<name>.sock",
        );
    }
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .unwrap_or(text);
    let valid = rest
        .rsplit_once(':')
        .is_some_and(|(host, number)| time_server(host).is_ok() && port_text(number).is_ok());
    flag(valid, "must be host:port, like 192.0.2.20:80")
}

// ---- paths ----

/// an absolute path of plain characters with no `..`, no `.` and no empty part.
pub fn absolute_path(text: &str) -> Check {
    if !text.starts_with('/') || text.len() > 4096 {
        return Err("must be an absolute path");
    }
    let parts_ok = text[1..].split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'+' | b'@')
            })
    });
    flag(
        text == "/" || parts_ok,
        "must be a plain absolute path: no .., no empty parts, no special characters",
    )
}

/// an absolute path inside `root` (or `root` itself).
pub fn path_within(text: &str, root: &str) -> Check {
    absolute_path(text)?;
    let inside = text == root
        || text
            .strip_prefix(root)
            .is_some_and(|rest| root.ends_with('/') || rest.starts_with('/'));
    flag(inside, "must be inside the folder this setting is kept in")
}

// ---- free text ----

fn has_control(text: &str, allowed: &[char]) -> bool {
    text.chars()
        .any(|c| c.is_control() && !allowed.contains(&c))
}

/// a single line of text: no control characters at all.
pub fn text_line(text: &str) -> Check {
    if text.chars().count() > LINE_LIMIT {
        return Err("is too long");
    }
    flag(!has_control(text, &[]), "must not hold control characters")
}

/// several lines of text: only the line feed and tab are allowed as control characters.
pub fn text_block(text: &str) -> Check {
    if text.chars().count() > BLOCK_LIMIT {
        return Err("is too long");
    }
    flag(
        !has_control(text, &['\n', '\t']),
        "must not hold control characters other than line feeds and tabs",
    )
}

/// the title of the sign-in screen: one line, at most 64 characters.
pub fn login_title(text: &str) -> Check {
    if text.chars().count() > crate::login_text::TITLE_LIMIT {
        return Err("is too long");
    }
    text_line(text)
}

/// the message of the sign-in screen: a few lines, at most 500 characters.
pub fn login_text(text: &str) -> Check {
    if text.chars().count() > crate::login_text::TEXT_LIMIT {
        return Err("is too long");
    }
    text_block(text)
}

/// a samba log level: a short single line of digits, lowercase letters, `_`, `:` and spaces
/// (`1`, `2 auth:3`).
pub fn log_level(text: &str) -> Check {
    flag(
        !text.is_empty()
            && text.len() <= 64
            && text.bytes().all(|b| {
                b.is_ascii_digit() || b.is_ascii_lowercase() || matches!(b, b'_' | b':' | b' ')
            }),
        "must be a samba log level like 1 or 2 auth:3",
    )
}

/// a secret value (a password): one line, no control characters. braces are fine, because
/// secrets are passed to ansible as unsafe text.
pub fn secret_text(text: &str) -> Check {
    flag(
        text.chars().count() <= LINE_LIMIT && !has_control(text, &[]),
        "must be one line of at most 1024 characters, with no control characters",
    )
}

/// what may name a place in a secrets file: `nt4-domain.users[dana].password`.
pub fn secret_path(text: &str) -> Check {
    flag(
        !text.is_empty()
            && text.len() <= 200
            && text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'[' | b']')
            }),
        "must be a settings path like nt4-domain.admin_password",
    )
}

// ---- the settings document ----

/// what a value in the settings must be.
enum Kind {
    /// a string that follows `rule`.
    Text(fn(&str) -> Check),
    /// a string that is one of these.
    OneOf(&'static [&'static str]),
    /// one of these strings, or the boolean `false` (yaml reads an unquoted `off` that way).
    OneOfOrOff(&'static [&'static str]),
    Flag,
    /// a whole number from `.0` to `.1`.
    Number(i64, i64),
    /// a list of at most 256 values that each follow the kind.
    List(&'static Kind),
    /// a value, or a list of values, that follow the kind.
    OneOrList(&'static Kind),
    /// a mapping; the keys named here are checked, others only get the general checks.
    Map(&'static [(&'static str, Kind)]),
    /// text of any sort (the general checks only).
    Free,
}

const LISTEN: Kind = Kind::Text(listen_target);
const NAME: Kind = Kind::Text(label);
const ADDRESS: Kind = Kind::Text(ipv4_address);
const INTERFACE: Kind = Kind::Text(interface_name);
const PASSWORD: Kind = Kind::Text(secret_text);

fn listen_target(text: &str) -> Check {
    flag(
        matches!(text, "host" | "internal") || ipv4_address(text).is_ok(),
        "must be host, internal or an ipv4 address",
    )
}

const RECORD: Kind = Kind::Map(&[("name", NAME), ("address", ADDRESS)]);
const STATIC_LEASE: Kind = Kind::Map(&[
    ("name", NAME),
    ("mac", Kind::Text(mac_address)),
    ("address", ADDRESS),
]);
const DHCP: Kind = Kind::Map(&[
    ("range", Kind::List(&ADDRESS)),
    ("lease", Kind::Text(lease)),
    ("hosts", Kind::List(&STATIC_LEASE)),
]);
const DNS: Kind = Kind::Map(&[
    ("zone", Kind::Text(domain)),
    ("records", Kind::List(&RECORD)),
    ("dhcp", DHCP),
]);
const NETWORK_ENTRY: Kind = Kind::Map(&[
    ("name", NAME),
    ("interface", INTERFACE),
    ("address", Kind::Text(ipv4_cidr)),
    ("mtu", Kind::Number(576, 9000)),
    ("zone", Kind::Text(domain)),
    ("records", Kind::List(&RECORD)),
    ("dhcp", DHCP),
]);
const NT4_USER: Kind = Kind::Map(&[
    ("name", Kind::Text(domain_user)),
    ("password", PASSWORD),
    ("admin", Kind::Flag),
    ("software_admin", Kind::Flag),
    ("state", Kind::OneOf(&["present", "absent"])),
]);
const WEB_PROXY_SITE: Kind = Kind::Map(&[
    ("host", Kind::Text(hostname)),
    ("upstream", Kind::Text(upstream)),
]);
const COMPONENT: Kind = Kind::Map(&[
    ("enabled", Kind::Flag),
    ("subcomponents", Kind::List(&NAME)),
    (
        "attach",
        Kind::List(&Kind::Map(&[("network", NAME), ("address", ADDRESS)])),
    ),
    ("mechanism", NAME),
]);

/// every section of the settings the playbooks read, with the fields they use. a top-level key
/// becomes an ansible extra variable, which outranks the playbooks' own variables, so a key that
/// is not named here is refused.
const SECTIONS: &[(&str, Kind)] = &[
    (
        "gateway",
        Kind::Map(&[
            ("motd", Kind::Text(text_block)),
            ("login_title", Kind::Text(login_title)),
            ("login_text", Kind::Text(login_text)),
            ("timezone", Kind::Text(time_zone)),
            ("ntp", Kind::List(&Kind::Text(time_server))),
            ("ntp_enabled", Kind::Flag),
        ]),
    ),
    (
        "docker",
        Kind::Map(&[
            ("host_interface", INTERFACE),
            ("internal_interface", INTERFACE),
            ("internal_address", Kind::Text(cidr)),
        ]),
    ),
    (
        "network",
        Kind::Map(&[
            ("host_interface", INTERFACE),
            ("internal_interface", INTERFACE),
        ]),
    ),
    ("networks", Kind::List(&NETWORK_ENTRY)),
    ("dns", DNS),
    (
        "nt4-domain",
        Kind::Map(&[
            ("mode", Kind::OneOf(&["domain", "workgroup"])),
            ("name", Kind::Text(netbios_name)),
            ("netbios_name", Kind::Text(netbios_name)),
            ("network", NAME),
            ("address", Kind::Text(ipv4_cidr)),
            ("admin_password", PASSWORD),
            ("roaming_profiles", Kind::Flag),
            ("software", Kind::Flag),
            ("lanman_auth", Kind::Flag),
            ("log_level", Kind::Text(log_level)),
            ("machines", Kind::List(&Kind::Text(machine_name))),
            ("users", Kind::List(&NT4_USER)),
        ]),
    ),
    (
        "web-proxy",
        Kind::Map(&[
            ("listen", Kind::OneOrList(&LISTEN)),
            ("tls", Kind::OneOfOrOff(&["off", "internal"])),
            ("sites", Kind::List(&WEB_PROXY_SITE)),
            (
                "webui",
                Kind::Map(&[
                    (
                        "tls",
                        Kind::OneOf(&["internal", "letsencrypt-dns", "cloudflare-origin"]),
                    ),
                    ("host", Kind::Text(hostname)),
                    ("port", Kind::Number(1, 65535)),
                    ("upstream", Kind::Text(upstream)),
                    ("proxy_user", Kind::Text(login_name)),
                ]),
            ),
            (
                "status",
                Kind::Map(&[("enabled", Kind::Flag), ("host", Kind::Text(hostname))]),
            ),
        ]),
    ),
    (
        "time-server",
        Kind::Map(&[
            ("pools", Kind::List(&Kind::Text(hostname))),
            ("servers", Kind::List(&Kind::Text(time_server))),
        ]),
    ),
    ("components", Kind::Free),
    ("access", Kind::Free),
    ("firewall", Kind::Free),
    (
        "remote-commands",
        Kind::Map(&[(
            "servers",
            Kind::List(&Kind::Map(&[
                ("name", NAME),
                ("address", Kind::Text(time_server)),
            ])),
        )]),
    ),
];

/// the only fields that may hold more than one line; every other text is one line, so a value
/// that reaches a template cannot add lines to a config file.
const MULTILINE: &[&str] = &["gateway.motd", "gateway.login_text"];

/// sections no playbook reads: they are kept in the settings file but never handed to ansible.
const NOT_FOR_PLAYBOOKS: &[&str] = &["access", "firewall", "remote-commands"];

/// the sections that hold named entries whose own keys are checked as components.
const COMPONENT_SECTION: &str = "components";

fn describe(kind: &Kind) -> &'static str {
    match kind {
        Kind::Text(_) | Kind::Free => "must be text",
        Kind::OneOf(_) | Kind::OneOfOrOff(_) => "must be one of the allowed values",
        Kind::Flag => "must be true or false",
        Kind::Number(..) => "must be a whole number in range",
        Kind::List(_) => "must be a list",
        Kind::OneOrList(_) => "must be a value or a list of values",
        Kind::Map(_) => "must be a mapping",
    }
}

fn check_text(path: &str, value: &Value, rule: fn(&str) -> Check) -> Result<(), Invalid> {
    match value {
        Value::String(text) => rule(text).map_err(|reason| Invalid::new(path, reason)),
        _ => Err(Invalid::new(path, "must be text")),
    }
}

fn check_choice(
    path: &str,
    value: &Value,
    choices: &[&str],
    off_allowed: bool,
) -> Result<(), Invalid> {
    let allowed = match value {
        Value::String(text) => choices.contains(&text.as_str()),
        Value::Bool(false) => off_allowed,
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(Invalid::new(
            path,
            format!("must be one of: {}", choices.join(", ")),
        ))
    }
}

fn check_number(path: &str, value: &Value, least: i64, most: i64) -> Result<(), Invalid> {
    match value.as_i64() {
        Some(number) if (least..=most).contains(&number) => Ok(()),
        _ => Err(Invalid::new(
            path,
            format!("must be a whole number from {least} to {most}"),
        )),
    }
}

fn check_list(path: &str, value: &Value, each: &Kind) -> Result<(), Invalid> {
    let Value::Sequence(items) = value else {
        return Err(Invalid::new(path, "must be a list"));
    };
    if items.len() > 256 {
        return Err(Invalid::new(path, "has too many entries"));
    }
    for (position, item) in items.iter().enumerate() {
        check_kind(&format!("{path}[{position}]"), item, each)?;
    }
    Ok(())
}

fn check_map(path: &str, value: &Value, fields: &[(&'static str, Kind)]) -> Result<(), Invalid> {
    let Value::Mapping(map) = value else {
        return Err(Invalid::new(path, "must be a mapping"));
    };
    for (key, kind) in fields {
        if let Some(found) = map.get(*key) {
            check_kind(&format!("{path}.{key}"), found, kind)?;
        }
    }
    for (key, found) in map {
        let name = key.as_str().unwrap_or_default();
        if !fields.iter().any(|(known, _)| *known == name) {
            check_words(&format!("{path}.{name}"), found)?;
        }
    }
    Ok(())
}

/// yaml 1.1 reads `yes`, `no`, `on` and `off` as booleans and ansible's templates treat the
/// text `no` as true, so a bare word like that in a field with no declared type is ambiguous.
fn bool_word(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Some(true),
        "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// refuse boolean-looking text anywhere in `value` (used where no type says what it is).
fn check_words(path: &str, value: &Value) -> Result<(), Invalid> {
    match value {
        Value::String(text) if bool_word(text).is_some() => Err(Invalid::new(
            path,
            "is the word yes, no, on, off, true or false, which yaml may read as a boolean: write true or false, or use another text",
        )),
        Value::Sequence(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(position, item)| check_words(&format!("{path}[{position}]"), item)),
        Value::Mapping(map) => map.iter().try_for_each(|(key, item)| {
            check_words(&format!("{path}.{}", key.as_str().unwrap_or_default()), item)
        }),
        _ => Ok(()),
    }
}

/// turn the text booleans of every flag field into real booleans, following the schema.
fn normalise_flags(value: &mut Value, kind: &Kind) {
    match (kind, value) {
        (Kind::Flag, value) => {
            if let Some(on) = value.as_str().and_then(bool_word) {
                *value = Value::Bool(on);
            }
        }
        (Kind::List(each) | Kind::OneOrList(each), Value::Sequence(items)) => {
            items
                .iter_mut()
                .for_each(|item| normalise_flags(item, each));
        }
        (Kind::OneOrList(each), value) => normalise_flags(value, each),
        (Kind::Map(fields), Value::Mapping(map)) => {
            for (name, field) in *fields {
                if let Some(found) = map.get_mut(*name) {
                    normalise_flags(found, field);
                }
            }
        }
        _ => {}
    }
}

fn check_kind(path: &str, value: &Value, kind: &Kind) -> Result<(), Invalid> {
    match kind {
        Kind::Text(rule) => check_text(path, value, *rule),
        Kind::OneOf(choices) => check_choice(path, value, choices, false),
        Kind::OneOfOrOff(choices) => check_choice(path, value, choices, true),
        Kind::Flag if value.is_bool() || value.as_str().and_then(bool_word).is_some() => Ok(()),
        Kind::Number(least, most) => check_number(path, value, *least, *most),
        Kind::List(each) => check_list(path, value, each),
        Kind::OneOrList(each) if value.is_sequence() => check_list(path, value, each),
        Kind::OneOrList(each) => check_kind(path, value, each),
        Kind::Map(fields) => check_map(path, value, fields),
        Kind::Free => Ok(()),
        Kind::Flag => Err(Invalid::new(path, describe(kind))),
    }
}

/// the general checks every value gets, typed or not: plain scalars and containers only (no yaml
/// tags), text without control characters (line feeds and tabs are fine in text), and a size
/// that keeps a hostile document from being expensive.
fn check_general(
    path: &str,
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), Invalid> {
    *nodes += 1;
    if depth > DEPTH_LIMIT || *nodes > NODE_LIMIT {
        return Err(Invalid::new(path, "is nested too deeply or too large"));
    }
    match value {
        Value::Tagged(_) => Err(Invalid::new(path, "must not use a yaml tag")),
        Value::String(text) => {
            let rule = if MULTILINE.contains(&path) {
                text_block
            } else {
                text_line
            };
            rule(text).map_err(|reason| Invalid::new(path, reason))
        }
        Value::Sequence(items) => items.iter().enumerate().try_for_each(|(position, item)| {
            check_general(&format!("{path}[{position}]"), item, depth + 1, nodes)
        }),
        Value::Mapping(map) => check_mapping_general(path, map, depth, nodes),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn check_mapping_general(
    path: &str,
    map: &Mapping,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), Invalid> {
    for (key, item) in map {
        let Some(name) = key.as_str() else {
            return Err(Invalid::new(path, "has a key that is not text"));
        };
        let here = if path.is_empty() {
            name.to_string()
        } else {
            format!("{path}.{name}")
        };
        text_line(name).map_err(|reason| Invalid::new(here.as_str(), reason))?;
        check_general(&here, item, depth + 1, nodes)?;
    }
    Ok(())
}

fn check_components(value: &Value) -> Result<(), Invalid> {
    let Value::Mapping(map) = value else {
        return Err(Invalid::new(COMPONENT_SECTION, "must be a mapping"));
    };
    for (key, entry) in map {
        let name = key.as_str().unwrap_or_default();
        label(name)
            .map_err(|reason| Invalid::new(format!("{COMPONENT_SECTION}.{name}"), reason))?;
        if !entry.is_null() {
            check_kind(&format!("{COMPONENT_SECTION}.{name}"), entry, &COMPONENT)?;
        }
    }
    Ok(())
}

fn section_kind(name: &str) -> Option<&'static Kind> {
    SECTIONS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, kind)| kind)
}

/// is `name` a section the playbooks read?
pub fn known_section(name: &str) -> bool {
    section_kind(name).is_some() || MODULES.iter().any(|module| module.id == name)
}

/// check a parsed settings document.
pub fn settings_value(document: &Value) -> Result<(), Invalid> {
    let map = match document {
        Value::Mapping(map) => map,
        Value::Null => return Ok(()),
        _ => return Err(Invalid::new("settings", "must be a mapping of sections")),
    };
    check_mapping_general("", map, 0, &mut 0)?;
    for (key, value) in map {
        let name = key.as_str().unwrap_or_default();
        if !known_section(name) {
            return Err(Invalid::new(
                name,
                "is not a settings section lanlord knows",
            ));
        }
        if name == COMPONENT_SECTION {
            check_components(value)?;
        } else if let Some(kind) = section_kind(name) {
            if !value.is_null() {
                check_kind(name, value, kind)?;
            }
        } else if !NOT_FOR_PLAYBOOKS.contains(&name) {
            check_words(name, value)?;
        }
    }
    Ok(())
}

/// check settings text before it is stored or applied.
pub fn settings_document(text: &str) -> Result<(), Invalid> {
    parse_document(text).map(|_| ())
}

/// parse and check settings text. anchors and aliases are refused on the text before any parse,
/// because a few lines of aliases expand into gigabytes.
fn parse_document(text: &str) -> Result<Value, Invalid> {
    if text.len() > 128 * 1024 {
        return Err(Invalid::new("settings", "is too large"));
    }
    // yaml 1.1 (the parser that builds the values) breaks lines at these, yaml 1.2 (the one that
    // looks for anchors) does not, so a comment could hide lines from the check
    if text.contains(['\u{85}', '\u{2028}', '\u{2029}']) {
        return Err(Invalid::new(
            "settings",
            "must not hold the line separators U+0085, U+2028 or U+2029",
        ));
    }
    match has_anchor_or_alias(text) {
        Ok(false) => {}
        Ok(true) => {
            return Err(Invalid::new(
                "settings",
                "must not use yaml anchors or aliases (& or *)",
            ))
        }
        Err(error) => {
            return Err(Invalid::new(
                "settings",
                format!("is not valid yaml: {error}"),
            ))
        }
    }
    let document: Value = serde_yaml_ng::from_str(text)
        .map_err(|error| Invalid::new("settings", format!("is not valid yaml: {error}")))?;
    settings_value(&document)?;
    Ok(document)
}

/// does the yaml text hold an anchor or an alias? read as parser events, which is what the yaml
/// library itself will see, and stopped at the first one: a few lines of aliases expand into
/// gigabytes, so this runs before the document is parsed into values. text that only holds an
/// `&` or a `*` is not an anchor and passes.
fn has_anchor_or_alias(text: &str) -> Result<bool, String> {
    for event in saphyr_parser::Parser::new_from_str(text) {
        match event {
            Ok((saphyr_parser::Event::Alias(_), _)) => return Ok(true),
            Ok((
                saphyr_parser::Event::Scalar(_, _, anchor, _)
                | saphyr_parser::Event::SequenceStart(anchor, _)
                | saphyr_parser::Event::MappingStart(anchor, _),
                _,
            )) if anchor != 0 => return Ok(true),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(false)
}

/// every string in `value` marked `!unsafe`, so that ansible keeps it as text and never runs it
/// through jinja. keys are not templated by ansible and stay as they are.
pub fn mark_unsafe(value: &Value) -> Value {
    match value {
        Value::String(_) => Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("unsafe"),
            value: value.clone(),
        })),
        Value::Sequence(items) => Value::Sequence(items.iter().map(mark_unsafe).collect()),
        Value::Mapping(map) => Value::Mapping(
            map.iter()
                .map(|(key, item)| (key.clone(), mark_unsafe(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// the yaml the playbooks get as `-e @settings.yml`: checked like [`settings_document`], with
/// every string marked unsafe. comments are not kept, nothing needs them there.
pub fn for_ansible(text: &str) -> Result<String, Invalid> {
    let mut document = match parse_document(text)? {
        Value::Null => Value::Mapping(Mapping::new()),
        other => other,
    };
    if let Value::Mapping(map) = &mut document {
        for name in NOT_FOR_PLAYBOOKS {
            map.remove(*name);
        }
        for (name, kind) in SECTIONS {
            if let Some(section) = map.get_mut(*name) {
                normalise_flags(section, kind);
            }
        }
        if let Some(Value::Mapping(components)) = map.get_mut(COMPONENT_SECTION) {
            components
                .values_mut()
                .for_each(|entry| normalise_flags(entry, &COMPONENT));
        }
    }
    serde_yaml_ng::to_string(&mark_unsafe(&document))
        .map_err(|error| Invalid::new("settings", format!("cannot be written: {error}")))
}

// ---- operations ----

fn invalid(field: &str, reason: &'static str) -> Invalid {
    Invalid::new(field, reason)
}

fn named(field: &str, rule: fn(&str) -> Check, text: &str) -> Result<(), Invalid> {
    rule(text).map_err(|reason| invalid(field, reason))
}

/// the key of an ssh public key line: one line, at most 8 KiB.
pub fn ssh_key(text: &str) -> Check {
    flag(
        text.len() <= 8192 && !has_control(text, &[]),
        "must be one line of at most 8192 characters, with no control characters",
    )
}

fn check_change(change: &UserChange) -> Result<(), Invalid> {
    match change {
        UserChange::SetPassword { name, password } => {
            named("name", domain_user, name)?;
            named("password", secret_text, password.expose())
        }
        UserChange::SetAdmin { name, .. }
        | UserChange::SetSoftwareAdmin { name, .. }
        | UserChange::Remove { name } => named("name", domain_user, name),
    }
}

fn check_access(access: &Access) -> Result<(), Invalid> {
    let name = |field: &str, text: &str| {
        let valid = !text.is_empty() && text.len() <= 64 && text_line(text).is_ok();
        valid
            .then_some(())
            .ok_or_else(|| invalid(field, "must be a plain name of 1 to 64 characters"))
    };
    for role in &access.roles {
        name("access.roles.name", &role.name)?;
    }
    for (user, roles) in &access.members {
        name("access.members", user)?;
        roles
            .iter()
            .try_for_each(|role| name("access.members", role))?;
    }
    for (user, profile) in &access.profiles {
        name("access.profiles", user)?;
        named("profile.full_name", text_line, &profile.full_name)?;
        named("profile.description", text_line, &profile.description)?;
    }
    Ok(())
}

fn check_clock(change: &crate::clock::Change) -> Result<(), Invalid> {
    if let Some(zone) = &change.timezone {
        named("timezone", time_zone, zone)?;
    }
    if let Some(time) = &change.time {
        crate::clock::valid_time(time)
            .map_err(|_| invalid("time", "must look like 2026-11-15 17:30:00"))?;
    }
    if let Some(ntp) = &change.ntp {
        if ntp.servers.len() > 8 {
            return Err(invalid("ntp.servers", "use at most eight time servers"));
        }
        for server in &ntp.servers {
            named("ntp.servers", time_server, server)?;
        }
    }
    Ok(())
}

fn check_save(plain: &str, updates: &[crate::operations::SecretUpdate]) -> Result<(), Invalid> {
    settings_document(plain)?;
    for (position, update) in updates.iter().enumerate() {
        named(
            &format!("updates[{position}].path"),
            secret_path,
            &update.path,
        )?;
        named(
            &format!("updates[{position}].value"),
            secret_text,
            update.value.expose(),
        )?;
    }
    Ok(())
}

fn check_command(command: &[String]) -> Result<(), Invalid> {
    let valid = !command.is_empty()
        && command.len() <= 256
        && command
            .iter()
            .all(|argument| argument.len() <= 4096 && !argument.contains('\0'));
    flag(
        valid,
        "must be a command of at most 256 words, none with a NUL",
    )
    .map_err(|reason| invalid("command", reason))
}

fn check_modules(ids: &[String]) -> Result<(), Invalid> {
    for (position, id) in ids.iter().enumerate() {
        if modules::find(id).is_none() {
            return Err(Invalid::new(
                format!("modules[{position}]"),
                "is not a module",
            ));
        }
    }
    Ok(())
}

/// a new network is checked on its own; the server checks the whole list again after the edit.
fn check_network_edit(edit: &crate::lan_edit::Edit) -> Result<(), Invalid> {
    use crate::lan_edit::Edit;
    match edit {
        Edit::Add(network) => {
            crate::lan_networks::validate(std::slice::from_ref(network), None, true)
                .into_iter()
                .next()
                .map_or(Ok(()), |e| Err(Invalid::new(e.path, e.message)))
        }
        Edit::Set { name, .. } | Edit::Remove(name) => named("name", label, name),
        Edit::LeaseAdd { network, .. } | Edit::LeaseRemove { network, .. } => {
            named("network", label, network)
        }
    }
}

/// check the arguments of an operation the way the server will, before anything runs. the first
/// field that is wrong is named.
pub fn operation(operation: &Operation) -> Result<(), Invalid> {
    match operation {
        Operation::Apply { modules } => check_modules(modules),
        Operation::SaveDocument { plain, updates } => check_save(plain, updates),
        Operation::ChangeUsers { changes } => changes.iter().try_for_each(check_change),
        Operation::ChangeNetworks { edit } => check_network_edit(edit),
        Operation::ResetMachine { name } => named("name", machine_name, name),
        Operation::CreateAdmin {
            name,
            password,
            ssh_key: key,
            ..
        } => {
            named("name", login_name, name)?;
            named("password", secret_text, password.expose())?;
            key.as_deref()
                .map_or(Ok(()), |key| named("ssh_key", ssh_key, key))
        }
        Operation::ReadLogon { name } => flag(
            crate::logon::valid_script_name(name),
            "is not a script name",
        )
        .map_err(|reason| invalid("name", reason)),
        Operation::WriteLogon { name, text } => {
            flag(
                crate::logon::valid_script_name(name),
                "is not a script name",
            )
            .map_err(|reason| invalid("name", reason))?;
            flag(!text.contains('\0'), "must not hold a NUL")
                .map_err(|reason| invalid("text", reason))
        }
        Operation::Run { command } => check_command(command),
        Operation::ClockZoneNow { zone } => named("zone", time_zone, zone),
        Operation::ClockApply { change } => check_clock(change),
        Operation::ChangePassword { old, new } => {
            named("old", secret_text, old.expose())?;
            named("new", secret_text, new.expose())
        }
        Operation::ResetPassword { user, new } => {
            named("user", login_name, user)?;
            named("new", secret_text, new.expose())
        }
        Operation::UpdateProfile { profile } => {
            named("full_name", text_line, &profile.full_name)?;
            named("description", text_line, &profile.description)
        }
        Operation::SaveAccess { access } => check_access(access),
        Operation::Subscribe { topic } => flag(
            topic.len() <= 64 && !has_control(topic, &[]),
            "is not a topic",
        )
        .map_err(|reason| invalid("topic", reason)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_yaml_ng::Value;

    use super::{
        absolute_path, cidr, domain, for_ansible, fqdn, hostname, interface_name, ip_address,
        ipv4_address, ipv4_cidr, ipv6_address, ipv6_cidr, label, login_name, mac_address,
        path_within, port, port_text, secret_path, secret_text, settings_document, text_block,
        text_line, time_server, time_zone, upstream, Check,
    };
    use crate::clock::{Change, NtpChange};
    use crate::operations::{Operation, SecretUpdate, UserChange};
    use crate::secret::Secret;

    fn table(rule: fn(&str) -> Check, good: &[&str], bad: &[&str]) {
        for value in good {
            assert_eq!(rule(value), Ok(()), "{value:?} should be accepted");
        }
        for value in bad {
            assert!(rule(value).is_err(), "{value:?} should be refused");
        }
    }

    #[test]
    fn names_follow_rfc_1123() {
        let long_label = "a".repeat(64);
        let long_name = format!("{}.com", ["abcdefghi"; 30].join("."));
        table(
            hostname,
            &[
                "gateway",
                "gw-1",
                "Host.Example.COM",
                "a.b.c",
                "xn--bcher-kva.example",
            ],
            &[
                "",
                "-gw",
                "gw-",
                "gw..example",
                ".example",
                "example.com.",
                "gw one",
                "gw\nexample",
                "gw\"x",
                "gw;id",
                "{{gw}}",
                "gw_1",
                "192.0.2.1",
                &long_label,
                &long_name,
            ],
        );
        table(
            domain,
            &["retronet", "lab.example.net"],
            &["", "lab/../x", "a b"],
        );
        table(fqdn, &["host.example.com"], &["host", "host.", "a..b"]);
        table(label, &["ntsrv1", "a"], &["", "a.b", "-a", "a-"]);
    }

    #[test]
    fn addresses_and_networks() {
        table(
            ipv4_address,
            &["192.0.2.1", "0.0.0.0", "255.255.255.255"],
            &[
                "",
                "192.0.2",
                "192.0.2.256",
                "192.0.2.1\n",
                "01.2.3.4",
                "192.0.2.1/24",
                "::1",
            ],
        );
        table(
            ipv6_address,
            &["2001:db8::1", "::1"],
            &["", "2001:db8::g", "192.0.2.1", "fe80::1%eth0", "[::1]"],
        );
        table(ip_address, &["192.0.2.1", "2001:db8::1"], &["host", ""]);
        table(
            ipv4_cidr,
            &["192.0.2.1/24", "192.0.2.0/0", "192.0.2.1/32"],
            &[
                "192.0.2.1",
                "192.0.2.1/33",
                "192.0.2.1/-1",
                "192.0.2.1/",
                "192.0.2.1/2 4",
                "x/24",
                "192.0.2.1/024x",
            ],
        );
        table(
            ipv6_cidr,
            &["2001:db8::/32", "2001:db8::1/128"],
            &["2001:db8::/129", "2001:db8::", "192.0.2.1/24"],
        );
        table(
            cidr,
            &["192.0.2.1/24", "2001:db8::/32"],
            &["192.0.2.1", "a/b"],
        );
        table(
            mac_address,
            &["aa:bb:cc:dd:ee:ff", "AA:BB:CC:DD:EE:FF"],
            &[
                "aa:bb:cc:dd:ee",
                "aa-bb-cc-dd-ee-ff",
                "aa:bb:cc:dd:ee:gg",
                "aa:bb:cc:dd:ee:ff\n",
            ],
        );
    }

    #[test]
    fn ports_run_from_one_to_65535() {
        for number in [1, 80, 8443, 65535] {
            assert_eq!(port(number), Ok(()));
        }
        for number in [-1, 0, 65536, i64::MAX] {
            assert!(port(number).is_err(), "{number}");
        }
        table(
            port_text,
            &["1", "65535"],
            &["0", "65536", "", "-1", "80a", "+80", "123456"],
        );
    }

    #[test]
    fn interface_names_fit_the_kernel() {
        table(
            interface_name,
            &[
                "eth0",
                "br-lan",
                "enp3s0f1",
                "vlan.10",
                "a_b",
                "123456789012345",
            ],
            &[
                "",
                ".",
                "..",
                "-eth0",
                "eth0;id",
                "eth 0",
                "eth0\n",
                "eth/0",
                "eth0:1",
                "$(id)",
                "1234567890123456",
                "{{x}}",
            ],
        );
    }

    #[test]
    fn zones_servers_and_accounts() {
        table(
            time_zone,
            &[
                "UTC",
                "Europe/Berlin",
                "America/Port-au-Prince",
                "Etc/GMT+5",
            ],
            &[
                "",
                "../etc/passwd",
                "Europe//Berlin",
                "/Europe",
                "Europe/",
                "a b",
                "Europe/Berlin\n",
                "x.y",
                &"a".repeat(65),
            ],
        );
        table(
            time_server,
            &[
                "pool.ntp.org",
                "192.0.2.123",
                "2001:db8::123",
                "ntp.example.net",
            ],
            &[
                "",
                "ntp server",
                "ntp;id",
                "ntp\n.example.com",
                "-bad.example",
            ],
        );
        table(
            login_name,
            &["lanlord", "_svc", "web-ui_1"],
            &[
                "",
                "Root",
                "1abc",
                "-x",
                "a b",
                "a:b",
                "a\nb",
                &"a".repeat(33),
            ],
        );
    }

    #[test]
    fn upstreams_are_sockets_or_host_and_port() {
        table(
            upstream,
            &[
                "unix//run/lanlord-web/web.sock",
                "192.0.2.20:80",
                "http://app.example.com:8080",
                "https://192.0.2.5:443",
            ],
            &[
                "",
                "unix//run/lanlord-web/../x.sock",
                "unix//run/other/web.sock",
                "192.0.2.20",
                "192.0.2.20:99999",
                "app.example.com:80/path",
                "ftp://192.0.2.20:80",
                "192.0.2.20:80\nroot",
            ],
        );
    }

    #[test]
    fn paths_are_absolute_and_stay_inside_their_root() {
        table(
            absolute_path,
            &["/", "/opt/lanlord/state", "/srv/status/index.html"],
            &[
                "",
                "opt/x",
                "/opt/../etc",
                "/opt//x",
                "/opt/./x",
                "/opt/x\n",
                "/opt/$(id)",
                "/opt/a b",
                "/opt/x/",
            ],
        );
        assert_eq!(path_within("/opt/lanlord/state", "/opt/lanlord"), Ok(()));
        assert_eq!(path_within("/opt/lanlord", "/opt/lanlord"), Ok(()));
        for outside in [
            "/opt/lanlordx/state",
            "/etc/passwd",
            "/opt/lanlord/../etc",
            "/opt",
        ] {
            assert!(path_within(outside, "/opt/lanlord").is_err(), "{outside}");
        }
    }

    #[test]
    fn free_text_has_no_control_characters() {
        table(
            text_line,
            &["welcome", "ünïcode", "braces {{ are text }}"],
            &["a\nb", "a\rb", "a\0b", "a\x1b[31m", &"x".repeat(1025)],
        );
        table(
            text_block,
            &["a\nb\tc", "{{ not run }}"],
            &["a\rb", "a\0b", "a\x07"],
        );
        table(
            secret_text,
            &["pa{{ss}}word", "p@ss w0rd"],
            &["a\nb", "a\0b"],
        );
        table(
            secret_path,
            &[
                "nt4-domain.admin_password",
                "nt4-domain.users[dana].password",
            ],
            &["", "a b", "a;b", "a\nb", "a/b"],
        );
    }

    fn refused(text: &str) -> String {
        settings_document(text).expect_err(text).to_string()
    }

    #[test]
    fn a_good_settings_document_is_accepted() {
        let text = "\
gateway:
  motd: \"welcome\\nto the lab\\n\"
  timezone: Europe/Berlin
  ntp: [pool.ntp.org, 192.0.2.123]
  ntp_enabled: true
networks:
  - name: retronet
    interface: eth1
    address: 192.0.2.1/24
    mtu: 1450
    zone: retronet
    records:
      - {name: ntsrv1, address: 192.0.2.10}
    dhcp:
      range: [192.0.2.100, 192.0.2.200]
      lease: 7d
      hosts:
        - {name: ntsrv1, mac: \"aa:bb:cc:dd:ee:ff\", address: 192.0.2.10}
nt4-domain:
  mode: domain
  name: RETRONET
  admin_password: \"{{ looks like jinja }}\"
  machines: [ws1, ws2]
  users:
    - {name: dana, password: pw, admin: true}
web-proxy:
  tls: off
  listen: [host, 192.0.2.5]
  sites:
    - {host: nostalgia.lan, upstream: \"192.0.2.20:80\"}
  webui: {tls: internal, host: gateway.example.com, port: 8443, proxy_user: root}
components:
  nt4-domain:
    enabled: true
    attach: [{network: retronet, address: 192.0.2.5}]
";
        assert_eq!(settings_document(text), Ok(()));
        assert_eq!(settings_document(""), Ok(()));
        assert_eq!(settings_document("{}"), Ok(()));
    }

    #[test]
    fn a_bad_settings_document_names_the_field() {
        let cases = [
            (
                "gateway:\n  timezone: \"../../etc/passwd\"\n",
                "gateway.timezone",
            ),
            (
                "gateway:\n  ntp: [\"a.example.com\\nserver evil\"]\n",
                "gateway.ntp[0]",
            ),
            ("gateway:\n  motd: \"x\\u001b[2Jy\"\n", "gateway.motd"),
            (
                "networks:\n  - {name: a, interface: \"eth0;id\"}\n",
                "networks[0].interface",
            ),
            (
                "networks:\n  - {name: a, address: 192.0.2.1/33}\n",
                "networks[0].address",
            ),
            ("networks:\n  - {name: a, mtu: 70000}\n", "networks[0].mtu"),
            ("networks:\n  - {name: \"a\\nb\"}\n", "networks[0].name"),
            ("dns:\n  zone: \"x\\\"; rm\"\n", "dns.zone"),
            (
                "dns:\n  dhcp:\n    hosts:\n      - {mac: nope}\n",
                "dns.dhcp.hosts[0].mac",
            ),
            (
                "nt4-domain:\n  users:\n    - {name: \"x --evil\"}\n",
                "nt4-domain.users[0].name",
            ),
            (
                "nt4-domain:\n  machines: [\"ws1; id\"]\n",
                "nt4-domain.machines[0]",
            ),
            (
                "web-proxy:\n  sites:\n    - {host: \"a.example.com\\nrespond x\"}\n",
                "web-proxy.sites[0].host",
            ),
            (
                "web-proxy:\n  webui: {port: 70000}\n",
                "web-proxy.webui.port",
            ),
            (
                "web-proxy:\n  webui: {port: \"8443\"}\n",
                "web-proxy.webui.port",
            ),
            (
                "web-proxy:\n  tls: \"internal } evil {\"\n",
                "web-proxy.tls",
            ),
            (
                "docker:\n  internal_address: \"192.0.2.1/24; id\"\n",
                "docker.internal_address",
            ),
            (
                "time-server:\n  pools: [\"{{ lookup('pipe','id') }}\"]\n",
                "time-server.pools[0]",
            ),
            ("components:\n  \"a b\": {}\n", "components.a b"),
            ("gateway: {motd: !unsafe x}\n", "gateway.motd"),
            ("image: evil\n", "image"),
            (
                "ansible_python_interpreter: /tmp/x\n",
                "ansible_python_interpreter",
            ),
            ("lanlord_image_version: x\n", "lanlord_image_version"),
            ("- a\n- b\n", "settings"),
            ("gateway: [a]\n", "gateway"),
            ("gateway: {", "settings"),
        ];
        for (text, field) in cases {
            let message = refused(text);
            assert!(
                message.starts_with(field),
                "{text:?} gave {message:?}, wanted {field}"
            );
            assert!(!message.contains('\n'), "one line: {message:?}");
        }
    }

    #[test]
    fn a_hostile_document_is_bounded() {
        let deep = format!("a:{}", " [".repeat(40));
        assert!(settings_document(&deep).is_err());
        let wide = format!("gateway:\n  ntp: [{}]\n", ["a.example.com"; 300].join(", "));
        assert!(refused(&wide).starts_with("gateway.ntp"));
        assert!(refused(&"#".repeat(200_000)).starts_with("settings"));
    }

    #[test]
    fn written_settings_mark_every_string_unsafe() {
        let text = "\
# comment
gateway:
  motd: \"{{ lookup('pipe', 'id') }} {% if x %}\"
  ntp: [pool.ntp.org]
  ntp_enabled: true
nt4-domain:
  admin_password: \"{# x #}\"
  users:
    - {name: dana, password: \"{{ 7*7 }}\"}
web-proxy:
  webui: {port: 8443}
";
        let written = for_ansible(text).unwrap();
        let back: Value = serde_yaml_ng::from_str(&written).unwrap();
        // every string leaf is tagged, no matter how deep; nothing else is
        fn leaves(value: &Value, found: &mut (usize, usize)) {
            match value {
                Value::Tagged(tagged) => {
                    assert_eq!(tagged.tag.to_string(), "!unsafe");
                    assert!(tagged.value.is_string(), "only strings are tagged");
                    found.0 += 1;
                }
                Value::String(text) => panic!("an untagged string: {text}"),
                Value::Sequence(items) => items.iter().for_each(|item| leaves(item, found)),
                Value::Mapping(map) => map.values().for_each(|item| leaves(item, found)),
                _ => found.1 += 1,
            }
        }
        let mut found = (0, 0);
        leaves(&back, &mut found);
        assert_eq!(
            found,
            (5, 2),
            "five strings tagged, a flag and a number left alone"
        );
        assert!(written.contains("!unsafe"), "{written}");
        assert!(!written.contains("# comment"));
        assert_eq!(for_ansible("").unwrap().trim(), "{}");
    }

    #[test]
    fn unlisted_text_is_one_line_and_samba_fields_are_typed() {
        // an unlisted field cannot carry a line break, a listed multi-line one can
        assert!(refused("web-proxy:\n  extra: \"a\\nb\"\n").starts_with("web-proxy.extra"));
        assert!(settings_document("gateway:\n  motd: \"a\\nb\"\n").is_ok());
        for bad in ["1\\n root preexec = id", "1;id", "A", "", "1 {{x}}"] {
            let text = format!("nt4-domain:\n  log_level: \"{bad}\"\n");
            assert!(refused(&text).starts_with("nt4-domain.log_level"), "{bad}");
        }
        for good in ["1", "2 auth:3", "3 smb2_credits:5"] {
            assert!(settings_document(&format!("nt4-domain:\n  log_level: \"{good}\"\n")).is_ok());
        }
        assert!(
            refused("nt4-domain:\n  lanman_auth: maybe\n").starts_with("nt4-domain.lanman_auth")
        );
    }

    #[test]
    fn yaml_one_one_booleans_become_real_booleans() {
        let written = for_ansible(
            "nt4-domain:\n  lanman_auth: no\n  software: On\n  roaming_profiles: false\n",
        )
        .unwrap();
        let back: Value = serde_yaml_ng::from_str(&written).unwrap();
        assert_eq!(back["nt4-domain"]["lanman_auth"], Value::Bool(false));
        assert_eq!(back["nt4-domain"]["software"], Value::Bool(true));
        assert_eq!(back["nt4-domain"]["roaming_profiles"], Value::Bool(false));
        // nested flags, inside lists, are normalised too
        let users = for_ansible("nt4-domain:\n  users:\n    - {name: dana, admin: yes}\n").unwrap();
        let back: Value = serde_yaml_ng::from_str(&users).unwrap();
        assert_eq!(back["nt4-domain"]["users"][0]["admin"], Value::Bool(true));
        // a bare word in a field with no type is refused, with the way out in the message
        let message = refused("web-proxy:\n  extra: no\n");
        assert!(message.starts_with("web-proxy.extra"), "{message}");
        assert!(message.contains("true or false"), "{message}");
        // sections no playbook reads are not checked for words, and are not handed to ansible
        let kept = for_ansible(
            "access:\n  profiles:\n    dana: {full_name: yes}\ngateway:\n  ntp_enabled: true\n",
        )
        .unwrap();
        assert!(
            !kept.contains("access") && kept.contains("ntp_enabled"),
            "{kept}"
        );
        // typed text keeps its word: a user may be called "on"
        assert!(settings_document("nt4-domain:\n  machines: [\"on\"]\n").is_ok());
    }

    #[test]
    fn nested_lists_and_maps_are_unsafe_and_booleans_stay_plain() {
        let written = for_ansible(
            "dns:\n  zone: example.com\n  records:\n    - {name: www, address: 192.0.2.7}\nweb-proxy:\n  listen: [host, internal]\n  status: {enabled: true}\n",
        )
        .unwrap();
        assert!(written.contains("zone: !unsafe example.com"), "{written}");
        assert!(written.contains("name: !unsafe www"), "{written}");
        assert!(written.contains("- !unsafe host"), "{written}");
        assert!(written.contains("enabled: true"), "{written}");
        assert!(!written.contains("!unsafe true"), "{written}");
    }

    #[test]
    fn anchors_and_aliases_are_refused_before_any_parse() {
        // a document that would expand to many gigabytes if it were parsed
        let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for step in 1..40 {
            let last = step - 1;
            bomb.push_str(&format!(
                "a{step}: &a{step} [*a{last}, *a{last}, *a{last}, *a{last}, *a{last}, *a{last}, *a{last}, *a{last}, *a{last}]\n"
            ));
        }
        assert!(bomb.len() < 128 * 1024);
        let started = std::time::Instant::now();
        let message = refused(&bomb);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(
            message.starts_with("settings") && message.contains("anchors"),
            "{message}"
        );
        for text in [
            "gateway: &g {ntp: [a.example.com]}\n",
            "gateway: *g\n",
            "gateway:\n  ntp: [*a]\n",
            "gateway:\n  ntp:\n    - &x a.example.com\n",
            "gateway:\n  ntp: [pool.example.com, {k: *a}]\n",
        ] {
            let message = refused(text);
            assert!(
                message.contains("anchors") || message.contains("not valid yaml"),
                "{text}: {message}"
            );
        }
        // the same marks inside quotes, comments and block text are only text
        for text in [
            "gateway:\n  motd: \"fish & *chips* &x\"\n",
            "gateway:\n  motd: 'a *b'\n",
            "gateway:\n  motd: |\n    *** welcome &here\n    line two\n  timezone: UTC\n",
            "gateway:\n  timezone: UTC # *alias &anchor\n",
            "gateway:\n  motd: Tom & Jerry * more\n",
        ] {
            assert!(
                settings_document(text).is_ok(),
                "{text}: {:?}",
                settings_document(text)
            );
        }
        // a block scalar ends where the indentation does: the sibling after it is read again
        assert!(refused("gateway:\n  motd: |\n    text\n  timezone: &z UTC\n").contains("anchors"));
        // inputs a hand-written scan once missed: each one expands under a real parser
        for text in [
            "k: a \"b\nx: &a [x,x]\ny: [*a,*a]\nz: c\"",
            "{\"a\":&x [x,x],\"b\":*x}",
            "k: [a |\n    , &a [x,x], *a, *a]",
            "k: &a x\nj: *a\n",
        ] {
            let message = refused(text);
            assert!(
                message.contains("anchors") || message.contains("not valid yaml"),
                "{text:?}: {message}"
            );
            assert!(settings_document(text).is_err());
        }
        // other line separators hide lines from one parser and not the other
        for separator in ['\u{85}', '\u{2028}', '\u{2029}'] {
            let hidden = format!("k: 1 # c{separator}j: &a [x, x]{separator}y: [*a, *a]\n");
            let message = refused(&hidden);
            assert!(message.contains("U+0085"), "{message}");
            let mut flat =
                format!("k: 1 # c{separator}a0: &a0 [x, x, x, x, x, x, x, x, x, x]{separator}");
            for step in 1..2000 {
                flat.push_str(&format!(
                    "a{step}: [*a0, *a0, *a0, *a0, *a0, *a0, *a0, *a0, *a0, *a0]{separator}"
                ));
            }
            assert!(flat.len() < 128 * 1024);
            let started = std::time::Instant::now();
            assert!(refused(&flat).contains("U+0085"));
            assert!(started.elapsed() < std::time::Duration::from_secs(1));
        }
        // plain text with & or * in it is not an anchor
        for text in [
            "gateway:\n  motd: cats & dogs\n",
            "gateway:\n  motd: a *b\n",
            "nt4-domain:\n  admin_password: \"p&ss *word\"\n",
            "nt4-domain:\n  users:\n    - {name: dana, password: a&b*c}\n",
        ] {
            assert!(
                settings_document(text).is_ok(),
                "{text}: {:?}",
                settings_document(text)
            );
            assert!(for_ansible(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn a_self_service_profile_never_blocks_a_save_or_an_apply() {
        for word in ["yes", "no", "on", "Off"] {
            let text = format!(
                "access:\n  profiles:\n    dana: {{full_name: {word}, email: {word}}}\ngateway:\n  ntp_enabled: true\n"
            );
            assert!(settings_document(&text).is_ok(), "{word}");
            let written = for_ansible(&text).unwrap();
            assert!(
                !written.contains("access") && !written.contains(word),
                "{written}"
            );
        }
    }

    #[test]
    fn a_field_name_cannot_forge_a_log_line() {
        let error = refused("gateway:\n  \"x\\nERR forged\": 1\n");
        assert!(!error.contains('\n'), "{error:?}");
        let error = super::Invalid::new("a\nb", "c\rd");
        assert_eq!(error.to_string(), "a\\nb: c\\rd");
    }

    #[test]
    fn a_document_that_fails_is_never_written() {
        let error = for_ansible("gateway:\n  timezone: \"../x\"\n").unwrap_err();
        assert_eq!(error.field, "gateway.timezone");
    }

    fn password(text: &str) -> Secret {
        Secret::new(text)
    }

    #[test]
    fn operations_are_checked_field_by_field() {
        let refused = [
            (
                Operation::Apply {
                    modules: vec!["dns".into(), "dns; id".into()],
                },
                "modules[1]",
            ),
            (
                Operation::SaveDocument {
                    plain: "gateway:\n  timezone: ../x\n".into(),
                    updates: vec![],
                },
                "gateway.timezone",
            ),
            (
                Operation::SaveDocument {
                    plain: String::new(),
                    updates: vec![SecretUpdate {
                        path: "a b".into(),
                        value: password("x"),
                    }],
                },
                "updates[0].path",
            ),
            (
                Operation::SaveDocument {
                    plain: String::new(),
                    updates: vec![SecretUpdate {
                        path: "a".into(),
                        value: password("x\ny"),
                    }],
                },
                "updates[0].value",
            ),
            (
                Operation::ResetMachine {
                    name: "ws1; id".into(),
                },
                "name",
            ),
            (
                Operation::ChangeUsers {
                    changes: vec![UserChange::Remove {
                        name: "x --y".into(),
                    }],
                },
                "name",
            ),
            (
                Operation::ChangeUsers {
                    changes: vec![UserChange::SetPassword {
                        name: "dana".into(),
                        password: password("a\nb"),
                    }],
                },
                "password",
            ),
            (
                Operation::CreateAdmin {
                    name: "Root".into(),
                    password: password("x"),
                    ssh_key: None,
                    passwordless_sudo: false,
                },
                "name",
            ),
            (
                Operation::CreateAdmin {
                    name: "ok".into(),
                    password: password("x"),
                    ssh_key: Some("ssh-ed25519 AAAA\nssh-rsa B".into()),
                    passwordless_sudo: false,
                },
                "ssh_key",
            ),
            (
                Operation::ReadLogon {
                    name: "../x".into(),
                },
                "name",
            ),
            (
                Operation::WriteLogon {
                    name: "x".into(),
                    text: "a\0b".into(),
                },
                "text",
            ),
            (Operation::Run { command: vec![] }, "command"),
            (
                Operation::Run {
                    command: vec!["ls\0".into()],
                },
                "command",
            ),
            (
                Operation::ClockZoneNow {
                    zone: "../x".into(),
                },
                "zone",
            ),
            (
                Operation::ClockApply {
                    change: Change {
                        timezone: Some("a b".into()),
                        ..Change::default()
                    },
                },
                "timezone",
            ),
            (
                Operation::ClockApply {
                    change: Change {
                        time: Some("yesterday".into()),
                        ..Change::default()
                    },
                },
                "time",
            ),
            (
                Operation::ClockApply {
                    change: Change {
                        ntp: Some(NtpChange {
                            enabled: true,
                            servers: vec!["a;b".into()],
                        }),
                        ..Change::default()
                    },
                },
                "ntp.servers",
            ),
            (
                Operation::ResetPassword {
                    user: "a b".into(),
                    new: password("x"),
                },
                "user",
            ),
            (
                Operation::Subscribe {
                    topic: "a\nb".into(),
                },
                "topic",
            ),
            (
                Operation::ChangeNetworks {
                    edit: Box::new(crate::lan_edit::Edit::Add(
                        crate::lan_networks::LanNetwork::new("lab", "eth2", "198.51.100.1"),
                    )),
                },
                "networks[0].address",
            ),
            (
                Operation::ChangeNetworks {
                    edit: Box::new(crate::lan_edit::Edit::Remove("a b".into())),
                },
                "name",
            ),
        ];
        for (operation, field) in refused {
            let error = operation.validate().expect_err(&format!("{operation:?}"));
            assert!(error.field.starts_with(field), "{operation:?} gave {error}");
        }
        let fine = [
            Operation::Apply {
                modules: vec!["dns".into()],
            },
            Operation::LoadDocument,
            Operation::ResetMachine { name: "ws1".into() },
            Operation::Run {
                command: vec!["uname".into(), "-a".into()],
            },
            Operation::ClockZoneNow {
                zone: "Europe/Berlin".into(),
            },
            Operation::ClockApply {
                change: Change {
                    timezone: Some("UTC".into()),
                    time: Some("2026-11-15 17:30:00".into()),
                    ntp: Some(NtpChange {
                        enabled: true,
                        servers: vec!["pool.ntp.org".into()],
                    }),
                },
            },
            Operation::SaveDocument {
                plain: "dns:\n  zone: lab\n".into(),
                updates: vec![],
            },
            Operation::ChangeNetworks {
                edit: Box::new(crate::lan_edit::Edit::Add(
                    crate::lan_networks::LanNetwork::new("lab", "eth2", "198.51.100.1/24"),
                )),
            },
        ];
        for operation in fine {
            assert_eq!(operation.validate(), Ok(()), "{operation:?}");
        }
    }
}
