//! your decrypted settings file, with typed access to the `dns` section.
//! everything outside the parts a front end edits is kept as it was.

use std::fmt;
use std::net::Ipv4Addr;

use anyhow::{anyhow, bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::format::{written_by, FORMAT_KEY, WRITTEN_BY_KEY};

/// the settings as a yaml mapping. text that came from a file is kept
/// verbatim, so saving it again keeps the comments, until a typed edit
/// replaces it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    root: Mapping,
    raw: Option<String>,
}

impl Settings {
    pub fn empty() -> Self {
        Self::default()
    }

    /// parse yaml text; the top level must be a mapping (or nothing at all).
    pub fn parse(text: &str) -> Result<Self> {
        let root = match serde_yaml_ng::from_str::<Value>(text)? {
            Value::Mapping(map) => map,
            Value::Null => Mapping::new(),
            _ => bail!("the settings must be a mapping of module names to their settings"),
        };
        Ok(Self {
            root,
            raw: Some(text.to_string()),
        })
    }

    /// the yaml to store: the original text when untouched, else a fresh dump.
    pub fn text(&self) -> String {
        match &self.raw {
            Some(raw) if !raw.trim().is_empty() => raw.clone(),
            _ => serde_yaml_ng::to_string(&self.root).unwrap_or_else(|_| "{}\n".into()),
        }
    }

    /// the paths of secret-looking values in these settings.
    pub fn secret_paths(&self) -> Vec<String> {
        secret_paths(&Value::Mapping(self.root.clone()))
    }

    pub fn section(&self, key: &str) -> Option<&Value> {
        self.root.get(key)
    }

    /// the names of the top-level sections.
    pub fn sections(&self) -> Vec<String> {
        self.root
            .keys()
            .filter(|k| !is_stamp_key(k))
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect()
    }

    /// replace one top-level section; the other sections are untouched.
    pub fn set_section(&mut self, key: &str, value: Value) {
        self.root.insert(Value::String(key.into()), value);
        self.raw = None;
    }

    /// drop one top-level section; the other sections are untouched.
    pub fn remove_section(&mut self, key: &str) {
        if self.root.remove(key).is_some() {
            self.raw = None;
        }
    }

    /// add a section that is not there yet, keeping the text (and its comments) as it was when
    /// the text can take it.
    pub fn add_section_keeping_text(&mut self, key: &str, value: Value) {
        let kept = self.raw.as_ref().and_then(|raw| {
            let mut one = Mapping::new();
            one.insert(Value::String(key.into()), value.clone());
            let added = serde_yaml_ng::to_string(&one).ok()?;
            let joined = format!("{}\n{added}", raw.trim_end());
            self.reads_as(&joined, key, &value).then_some(joined)
        });
        self.root.insert(Value::String(key.into()), value);
        self.raw = kept;
    }

    /// whether `text` is a mapping that is these settings with `key` set to `value`.
    fn reads_as(&self, text: &str, key: &str, value: &Value) -> bool {
        let mut expected = self.root.clone();
        expected.insert(Value::String(key.into()), value.clone());
        matches!(
            serde_yaml_ng::from_str::<Value>(text),
            Ok(Value::Mapping(map)) if map == expected
        )
    }

    /// the format version the settings say they are in, and the lanlord that wrote them;
    /// `None` for the version when they say nothing (version 1).
    pub fn format(&self) -> Result<(Option<u32>, Option<String>)> {
        let version = match self.root.get(FORMAT_KEY) {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => Some(
                n.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| anyhow!("{FORMAT_KEY} must be a whole number from 1"))?,
            ),
            Some(_) => bail!("{FORMAT_KEY} must be a whole number from 1"),
        };
        let by = self.root.get(WRITTEN_BY_KEY).and_then(text_of);
        Ok((version, by))
    }

    /// say these settings are in format `version`, written by this lanlord. the stamp goes at
    /// the top; the rest of the text is kept.
    pub fn stamp(&mut self, version: u32) {
        let by = written_by();
        let mut root = Mapping::new();
        root.insert(
            Value::String(FORMAT_KEY.into()),
            Value::Number(version.into()),
        );
        root.insert(
            Value::String(WRITTEN_BY_KEY.into()),
            Value::String(by.into()),
        );
        for (key, value) in &self.root {
            if !is_stamp_key(key) {
                root.insert(key.clone(), value.clone());
            }
        }
        let raw = self.raw.take().filter(|raw| !raw.trim().is_empty());
        self.raw = raw.and_then(|raw| {
            let body: String = raw
                .lines()
                .filter(|line| {
                    !line.starts_with(&format!("{FORMAT_KEY}:"))
                        && !line.starts_with(&format!("{WRITTEN_BY_KEY}:"))
                })
                .map(|line| format!("{line}\n"))
                .collect();
            let text = format!("{FORMAT_KEY}: {version}\n{WRITTEN_BY_KEY}: \"{by}\"\n{body}");
            matches!(
                serde_yaml_ng::from_str::<Value>(&text),
                Ok(Value::Mapping(map)) if map == root
            )
            .then_some(text)
        });
        self.root = root;
    }
}

fn is_stamp_key(key: &Value) -> bool {
    matches!(key.as_str(), Some(FORMAT_KEY) | Some(WRITTEN_BY_KEY))
}

/// the key names whose values are secrets: they live encrypted on the
/// gateway and never in the plain settings.
pub const SECRET_KEYS: &[&str] = &["password", "admin_password", "token", "api_token", "secret"];

pub fn is_secret_key(name: &str) -> bool {
    SECRET_KEYS.contains(&name)
}

fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

fn name_of(value: &Value) -> Option<&str> {
    value.get("name").and_then(Value::as_str)
}

#[doc(hidden)]
pub fn parse_value(bytes: &[u8]) -> Result<Value> {
    match serde_yaml_ng::from_slice::<Value>(bytes)? {
        Value::Mapping(map) => Ok(Value::Mapping(map)),
        Value::Null => Ok(Value::Mapping(Mapping::new())),
        _ => bail!("the settings must be a mapping of module names to their settings"),
    }
}

#[doc(hidden)]
pub fn text_of_bytes(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| anyhow!("the stored settings are not text"))
}

#[doc(hidden)]
pub fn dump(value: &Value) -> Result<String> {
    Ok(serde_yaml_ng::to_string(value)?)
}

/// split a full settings document into (plain, secrets). a secret keeps its
/// key path; list entries with a `name` are tagged with it so they can be
/// matched again. secrets inside list entries without a name stay in the
/// plain part, where saving refuses them.
pub fn split(document: &Value) -> (Value, Value) {
    let (plain, secrets) = split_value(document);
    (plain, secrets.unwrap_or(Value::Mapping(Mapping::new())))
}

fn split_value(value: &Value) -> (Value, Option<Value>) {
    match value {
        Value::Mapping(map) => {
            let (mut plain, mut secrets) = (Mapping::new(), Mapping::new());
            for (key, item) in map {
                if key.as_str().is_some_and(is_secret_key) && is_scalar(item) {
                    secrets.insert(key.clone(), item.clone());
                    continue;
                }
                let (kept, found) = split_value(item);
                plain.insert(key.clone(), kept);
                if let Some(found) = found {
                    secrets.insert(key.clone(), found);
                }
            }
            let secrets = (!secrets.is_empty()).then_some(Value::Mapping(secrets));
            (Value::Mapping(plain), secrets)
        }
        Value::Sequence(items) => {
            let (mut plain, mut secrets) = (Vec::new(), Vec::new());
            for item in items {
                let Some(name) = name_of(item) else {
                    plain.push(item.clone());
                    continue;
                };
                let (kept, found) = split_value(item);
                plain.push(kept);
                if let Some(Value::Mapping(found)) = found {
                    let mut tagged = Mapping::new();
                    tagged.insert("name".into(), Value::String(name.into()));
                    tagged.extend(found);
                    secrets.push(Value::Mapping(tagged));
                }
            }
            let secrets = (!secrets.is_empty()).then_some(Value::Sequence(secrets));
            (Value::Sequence(plain), secrets)
        }
        other => (other.clone(), None),
    }
}

/// lay `secrets` over `plain`, key by key; list entries match by `name`
/// (or by position when they have none) and unmatched secrets are dropped.
pub fn merge(plain: &Value, secrets: &Value) -> Value {
    match (plain, secrets) {
        (Value::Mapping(base), Value::Mapping(over)) => {
            let mut out = base.clone();
            for (key, secret) in over {
                let merged = match out.get(key) {
                    Some(existing) => merge(existing, secret),
                    None => secret.clone(),
                };
                out.insert(key.clone(), merged);
            }
            Value::Mapping(out)
        }
        (Value::Sequence(base), Value::Sequence(over)) => {
            let mut out = base.clone();
            for (position, secret) in over.iter().enumerate() {
                let target = match name_of(secret) {
                    Some(name) => out.iter().position(|item| name_of(item) == Some(name)),
                    None => (position < out.len()).then_some(position),
                };
                if let Some(at) = target {
                    out[at] = merge(&out[at], secret);
                }
            }
            Value::Sequence(out)
        }
        (_, Value::Null) => plain.clone(),
        (_, secret) => secret.clone(),
    }
}

/// the paths of every secret value in `value`, like
/// `nt4-domain.admin_password` or `nt4-domain.users[dana].password`.
pub fn secret_paths(value: &Value) -> Vec<String> {
    secret_values(value)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// every secret in `value` with its path and its text, for sending a whole settings file to
/// a gateway as secret updates.
pub fn secret_values(value: &Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    collect_secret_values(value, "", &mut found);
    found
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

fn collect_secret_values(value: &Value, prefix: &str, found: &mut Vec<(String, String)>) {
    match value {
        Value::Mapping(map) => {
            for (key, item) in map {
                let Some(key) = key.as_str() else { continue };
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                if is_secret_key(key) && is_scalar(item) {
                    found.push((path, scalar_text(item)));
                } else {
                    collect_secret_values(item, &path, found);
                }
            }
        }
        Value::Sequence(items) => {
            for (i, item) in items.iter().enumerate() {
                let entry = name_of(item).map_or_else(|| i.to_string(), String::from);
                collect_secret_values(item, &format!("{prefix}[{entry}]"), found);
            }
        }
        _ => {}
    }
}

/// a secret the settings call for and whether it has a value; the value
/// itself is never read back.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SecretField {
    pub path: String,
    pub set: bool,
}

/// the secrets the plain settings imply (the nt4 admin password, a password
/// per named user) together with those already stored, set or not.
pub fn secret_fields(plain_text: &str, stored: &[String]) -> Vec<SecretField> {
    let mut paths: Vec<String> = stored.to_vec();
    if let Ok(Value::Mapping(root)) = parse_value(plain_text.as_bytes()) {
        for (section, body) in &root {
            let (Some(section), Value::Mapping(body)) = (section.as_str(), body) else {
                continue;
            };
            if section == "nt4-domain" {
                paths.push(format!("{section}.admin_password"));
            }
            if let Some(Value::Sequence(users)) = body.get("users") {
                for name in users.iter().filter_map(name_of) {
                    paths.push(format!("{section}.users[{name}].password"));
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| SecretField {
            set: stored.contains(&path),
            path,
        })
        .collect()
}

enum Step {
    Key(String),
    Item(String),
}

fn parse_path(path: &str) -> Result<Vec<Step>> {
    let (mut steps, mut word) = (Vec::new(), String::new());
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        match c {
            '.' | '[' => {
                if !word.is_empty() {
                    steps.push(Step::Key(std::mem::take(&mut word)));
                }
                if c == '[' {
                    let name: String = chars.by_ref().take_while(|c| *c != ']').collect();
                    if name.is_empty() {
                        bail!("{path:?} has an empty [name]");
                    }
                    steps.push(Step::Item(name));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        steps.push(Step::Key(word));
    }
    match steps.last() {
        Some(Step::Key(key)) if is_secret_key(key) => Ok(steps),
        _ => bail!("{path:?} is not a secret: it must end in one of {SECRET_KEYS:?}"),
    }
}

/// the top-level section a secret `path` writes into: the first step of the very parse the
/// writer uses, so what is checked is what is changed. a path that does not start with a key
/// (or is not a secret path at all) has none.
pub fn secret_section(path: &str) -> Result<String> {
    match parse_path(path)?.into_iter().next() {
        Some(Step::Key(key)) => Ok(key),
        _ => bail!("{path:?} does not start with a section"),
    }
}

fn set_at(value: &mut Value, steps: &[Step], secret: &str) -> Result<()> {
    let Some((step, rest)) = steps.split_first() else {
        *value = Value::String(secret.into());
        return Ok(());
    };
    match step {
        Step::Key(key) => {
            if value.is_null() {
                *value = Value::Mapping(Mapping::new());
            }
            let map = value.as_mapping_mut().ok_or_else(|| {
                anyhow!("{key:?} is not a section here: a list or value is stored there")
            })?;
            let entry = map.entry(Value::String(key.clone())).or_insert(Value::Null);
            set_at(entry, rest, secret)
        }
        Step::Item(name) => {
            if value.is_null() {
                *value = Value::Sequence(Vec::new());
            }
            let list = value.as_sequence_mut().ok_or_else(|| {
                anyhow!("[{name}] is not a list entry here: a section or value is stored there")
            })?;
            let at = match list.iter().position(|item| name_of(item) == Some(name)) {
                Some(at) => at,
                None => {
                    let mut entry = Mapping::new();
                    entry.insert("name".into(), Value::String(name.clone()));
                    list.push(Value::Mapping(entry));
                    list.len() - 1
                }
            };
            set_at(&mut list[at], rest, secret)
        }
    }
}

fn remove_at(value: &mut Value, steps: &[Step]) {
    let Some((step, rest)) = steps.split_first() else {
        return;
    };
    match step {
        Step::Key(key) => {
            let Some(map) = value.as_mapping_mut() else {
                return;
            };
            if rest.is_empty() {
                map.remove(key.as_str());
            } else if let Some(child) = map.get_mut(key.as_str()) {
                remove_at(child, rest);
            }
        }
        Step::Item(name) => {
            let Some(list) = value.as_sequence_mut() else {
                return;
            };
            let Some(at) = list.iter().position(|item| name_of(item) == Some(name)) else {
                return;
            };
            remove_at(&mut list[at], rest);
            // an entry with nothing left but its name holds no secrets
            if list[at].as_mapping().is_some_and(|map| map.len() <= 1) {
                list.remove(at);
            }
        }
    }
}

/// forget the secret at `path` in the secrets document, if it is there.
pub fn remove_secret(secrets: &mut Value, path: &str) -> Result<()> {
    remove_at(secrets, &parse_path(path)?);
    Ok(())
}

/// write `secret` at `path` in the secrets document, creating what is missing. a path that
/// does not fit what is stored (a key over a list, an item over a mapping, either over a
/// value) is refused and the document is left as it was.
pub fn set_secret(secrets: &mut Value, path: &str, secret: &str) -> Result<()> {
    let steps = parse_path(path)?;
    let mut next = secrets.clone();
    set_at(&mut next, &steps, secret).map_err(|e| anyhow!("cannot set {path:?}: {e}"))?;
    *secrets = next;
    Ok(())
}

/// the settings as stored on a gateway, as the editor needs them.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Document {
    /// the plain settings text, exactly as stored.
    pub plain: String,
    /// the paths of the secrets that have a value (never the values).
    pub secrets_set: Vec<String>,
    /// true when only the old all-in-one encrypted file exists; the next
    /// save moves its secrets to the encrypted file and the rest to the plain one.
    pub legacy: bool,
    /// true when the stored secrets are sealed to a key this gateway does not hold (an older
    /// client's key sealed them): move them to the gateway once, with the old key, to use them.
    #[serde(default)]
    pub needs_move: bool,
}

impl Document {
    /// the secrets to show next to the plain text.
    pub fn secret_fields(&self) -> Vec<SecretField> {
        secret_fields(&self.plain, &self.secrets_set)
    }
}

/// an ipv4 network, always stored with its host bits cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Subnet {
    pub network: Ipv4Addr,
    pub prefix: u8,
}

impl Subnet {
    /// the network `address/prefix` is on; a prefix over 32 counts as 32.
    pub fn new(address: Ipv4Addr, prefix: u8) -> Self {
        let prefix = prefix.min(32);
        Self {
            network: Ipv4Addr::from(u32::from(address) & Self::mask(prefix)),
            prefix,
        }
    }

    fn mask(prefix: u8) -> u32 {
        u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
    }

    pub fn contains(&self, address: Ipv4Addr) -> bool {
        u32::from(address) & Self::mask(self.prefix) == u32::from(self.network)
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.network) | !Self::mask(self.prefix))
    }
}

impl fmt::Display for Subnet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

/// something wrong with a setting, and where it is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ValidationError {
    /// like `dns.records[1].address`.
    pub path: String,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StaticLease {
    pub name: String,
    pub mac: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Dhcp {
    pub range_start: String,
    pub range_end: String,
    pub lease: String,
    pub hosts: Vec<StaticLease>,
}

impl Default for Dhcp {
    fn default() -> Self {
        Self {
            range_start: String::new(),
            range_end: String::new(),
            lease: DEFAULT_LEASE.into(),
            hosts: Vec::new(),
        }
    }
}

const DEFAULT_ZONE: &str = "retronet";
pub const DEFAULT_LEASE: &str = "7d";

/// the `dns` section; see ansible/playbooks/dns.yml for what each part does.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DnsSettings {
    pub zone: String,
    pub records: Vec<Record>,
    pub dhcp: Option<Dhcp>,
}

impl Default for DnsSettings {
    fn default() -> Self {
        Self {
            zone: DEFAULT_ZONE.into(),
            records: Vec::new(),
            dhcp: None,
        }
    }
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn field(map: &Mapping, key: &str, path: &str) -> Result<String> {
    map.get(key)
        .and_then(text_of)
        .ok_or_else(|| anyhow!("{path}.{key} is missing or not a value"))
}

fn entries<'a>(map: &'a Mapping, key: &str, path: &str) -> Result<Vec<&'a Mapping>> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Sequence(items)) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                item.as_mapping()
                    .ok_or_else(|| anyhow!("{path}.{key}[{i}] is not a mapping"))
            })
            .collect(),
        Some(_) => bail!("{path}.{key} is not a list"),
    }
}

fn parse_dhcp(map: &Mapping) -> Result<Dhcp> {
    let range = match map.get("range") {
        Some(Value::Sequence(r)) if r.len() == 2 => r,
        _ => bail!("dns.dhcp.range needs two addresses"),
    };
    let end = |i: usize| {
        text_of(&range[i]).ok_or_else(|| anyhow!("dns.dhcp.range[{i}] is not an address"))
    };
    let hosts = entries(map, "hosts", "dns.dhcp")?
        .into_iter()
        .map(|h| {
            Ok(StaticLease {
                name: field(h, "name", "dns.dhcp.hosts")?,
                mac: field(h, "mac", "dns.dhcp.hosts")?,
                address: field(h, "address", "dns.dhcp.hosts")?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Dhcp {
        range_start: end(0)?,
        range_end: end(1)?,
        lease: map
            .get("lease")
            .and_then(text_of)
            .unwrap_or_else(|| DEFAULT_LEASE.into()),
        hosts,
    })
}

impl DnsSettings {
    /// read the `dns` section; a missing or empty one gives the defaults.
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        let map = match settings.section("dns") {
            None | Some(Value::Null) => return Ok(Self::default()),
            Some(Value::Mapping(map)) => map,
            Some(_) => bail!("dns must be a mapping"),
        };
        let records = entries(map, "records", "dns")?
            .into_iter()
            .map(|r| {
                Ok(Record {
                    name: field(r, "name", "dns.records")?,
                    address: field(r, "address", "dns.records")?,
                })
            })
            .collect::<Result<_>>()?;
        let dhcp = match map.get("dhcp") {
            None | Some(Value::Null) => None,
            Some(Value::Mapping(d)) => Some(parse_dhcp(d)?),
            Some(_) => bail!("dns.dhcp must be a mapping"),
        };
        Ok(Self {
            zone: map
                .get("zone")
                .and_then(text_of)
                .unwrap_or_else(|| DEFAULT_ZONE.into()),
            records,
            dhcp,
        })
    }

    /// write this into the `dns` section of `settings`; other sections and
    /// keys this type does not know about stay as they are.
    pub fn apply_to(&self, settings: &mut Settings) {
        let mut dns = match settings.section("dns") {
            Some(Value::Mapping(map)) => map.clone(),
            _ => Mapping::new(),
        };
        set(&mut dns, "zone", Value::String(self.zone.clone()));
        let records = self
            .records
            .iter()
            .map(|r| pairs(&[("name", &r.name), ("address", &r.address)]))
            .collect();
        set(&mut dns, "records", Value::Sequence(records));
        match &self.dhcp {
            Some(dhcp) => {
                let mut map = match dns.get("dhcp") {
                    Some(Value::Mapping(map)) => map.clone(),
                    _ => Mapping::new(),
                };
                let range = [&dhcp.range_start, &dhcp.range_end]
                    .map(|a| Value::String(a.clone()))
                    .to_vec();
                set(&mut map, "range", Value::Sequence(range));
                set(&mut map, "lease", Value::String(dhcp.lease.clone()));
                let hosts = dhcp
                    .hosts
                    .iter()
                    .map(|h| pairs(&[("name", &h.name), ("mac", &h.mac), ("address", &h.address)]))
                    .collect();
                set(&mut map, "hosts", Value::Sequence(hosts));
                set(&mut dns, "dhcp", Value::Mapping(map));
            }
            None => {
                dns.remove("dhcp");
            }
        }
        settings.set_section("dns", Value::Mapping(dns));
    }

    /// everything wrong with these settings, for the networks the gateway is on, each as its
    /// subnet and the gateway's own address there. an address is fine inside any of them.
    /// empty means fine.
    pub fn validate(&self, networks: &[(Subnet, Ipv4Addr)]) -> Vec<ValidationError> {
        let mut check = Check {
            errors: Vec::new(),
            networks: networks.to_vec(),
        };
        if !valid_label(&self.zone) {
            check.fail("dns.zone", "must be a plain label like retronet");
        }
        for (i, record) in self.records.iter().enumerate() {
            let path = format!("dns.records[{i}]");
            check.name(&format!("{path}.name"), &record.name);
            check.address(&format!("{path}.address"), &record.address);
        }
        if let Some(dhcp) = &self.dhcp {
            check.dhcp(dhcp);
        }
        check.errors
    }
}

fn set(map: &mut Mapping, key: &str, value: Value) {
    map.insert(Value::String(key.into()), value);
}

fn pairs(items: &[(&str, &String)]) -> Value {
    let mut map = Mapping::new();
    for (key, value) in items {
        set(&mut map, key, Value::String((*value).clone()));
    }
    Value::Mapping(map)
}

/// a dns label as the dns playbook accepts it, after lowercasing.
pub fn valid_label(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let edge = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let (Some(first), Some(last)) = (name.chars().next(), name.chars().last()) else {
        return false;
    };
    name.len() <= 63 && edge(first) && edge(last) && name.chars().all(|c| edge(c) || c == '-')
}

/// a mac like aa:bb:cc:dd:ee:ff, either case.
pub fn valid_mac(mac: &str) -> bool {
    let parts: Vec<&str> = mac.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// 30m, 12h, 7d or infinite.
pub fn valid_lease(lease: &str) -> bool {
    let lease = lease.to_ascii_lowercase();
    if lease == "infinite" {
        return true;
    }
    match lease.strip_suffix(['m', 'h', 'd']) {
        Some(digits) => {
            (1..=5).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

struct Check {
    errors: Vec<ValidationError>,
    networks: Vec<(Subnet, Ipv4Addr)>,
}

impl Check {
    fn fail(&mut self, path: &str, message: impl Into<String>) {
        self.errors.push(ValidationError {
            path: path.into(),
            message: message.into(),
        });
    }

    fn name(&mut self, path: &str, name: &str) {
        if !valid_label(name) {
            self.fail(path, "must be a plain name: letters, digits and dashes");
        }
    }

    /// an ipv4 address inside one of the networks that is not a gateway, the network or the
    /// broadcast.
    fn address(&mut self, path: &str, text: &str) -> Option<Ipv4Addr> {
        let Ok(address) = text.trim().parse::<Ipv4Addr>() else {
            self.fail(path, format!("{text:?} is not an ipv4 address"));
            return None;
        };
        let Some((subnet, _)) = self.networks.iter().find(|(s, _)| s.contains(address)) else {
            let message = match self.networks.as_slice() {
                [(only, _)] => format!("{address} is outside {only}"),
                _ => format!("{address} is outside every network"),
            };
            self.fail(path, message);
            return Some(address);
        };
        let subnet = *subnet;
        if self.networks.iter().any(|(_, gateway)| *gateway == address) {
            self.fail(path, format!("{address} is the gateway's own address"));
        } else if subnet.prefix < 31 && (address == subnet.network || address == subnet.broadcast())
        {
            self.fail(
                path,
                format!("{address} is the network or broadcast address of {subnet}"),
            );
        }
        Some(address)
    }

    fn dhcp(&mut self, dhcp: &Dhcp) {
        let start = self.address("dns.dhcp.range[0]", &dhcp.range_start);
        let end = self.address("dns.dhcp.range[1]", &dhcp.range_end);
        if let (Some(start), Some(end)) = (start, end) {
            if start > end {
                self.fail("dns.dhcp.range", "the start must not be after the end");
            }
        }
        if !valid_lease(&dhcp.lease) {
            self.fail("dns.dhcp.lease", "must look like 30m, 12h, 7d or infinite");
        }
        let mut seen_addresses: Vec<Ipv4Addr> = Vec::new();
        let mut seen_macs: Vec<String> = Vec::new();
        for (i, host) in dhcp.hosts.iter().enumerate() {
            let path = format!("dns.dhcp.hosts[{i}]");
            self.name(&format!("{path}.name"), &host.name);
            if !valid_mac(&host.mac) {
                self.fail(&format!("{path}.mac"), "must look like aa:bb:cc:dd:ee:ff");
            } else if seen_macs.contains(&host.mac.to_ascii_lowercase()) {
                self.fail(
                    &format!("{path}.mac"),
                    format!("{} is used twice", host.mac),
                );
            } else {
                seen_macs.push(host.mac.to_ascii_lowercase());
            }
            if let Some(address) = self.address(&format!("{path}.address"), &host.address) {
                if seen_addresses.contains(&address) {
                    self.fail(
                        &format!("{path}.address"),
                        format!("{address} is used twice"),
                    );
                }
                seen_addresses.push(address);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_secret_path_names_the_section_it_writes_into() {
        assert_eq!(
            secret_section("nt4-domain.admin_password").unwrap(),
            "nt4-domain"
        );
        assert_eq!(
            secret_section(".nt4-domain.admin_password").unwrap(),
            "nt4-domain"
        );
        assert_eq!(
            secret_section("nt4-domain[x].password").unwrap(),
            "nt4-domain"
        );
        assert!(secret_section("[x].password").is_err());
        assert!(secret_section("").is_err());
        assert!(secret_section("nt4-domain.name").is_err());
    }

    use super::*;

    const GATEWAY: Ipv4Addr = Ipv4Addr::new(172, 16, 120, 1);

    fn subnet() -> Subnet {
        Subnet::new(GATEWAY, 24)
    }

    fn errors(dns: &DnsSettings) -> Vec<String> {
        dns.validate(&[(subnet(), GATEWAY)])
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn good() -> DnsSettings {
        DnsSettings {
            zone: "retronet".into(),
            records: vec![Record {
                name: "ntsrv1".into(),
                address: "172.16.120.10".into(),
            }],
            dhcp: Some(Dhcp {
                range_start: "172.16.120.100".into(),
                range_end: "172.16.120.200".into(),
                lease: "7d".into(),
                hosts: vec![StaticLease {
                    name: "ntsrv1".into(),
                    mac: "aa:bb:cc:dd:ee:ff".into(),
                    address: "172.16.120.10".into(),
                }],
            }),
        }
    }

    #[test]
    fn a_good_configuration_has_no_errors() {
        assert!(errors(&good()).is_empty());
        assert!(errors(&DnsSettings::default()).is_empty());
    }

    #[test]
    fn zone_and_names_must_be_plain_labels() {
        let mut dns = good();
        dns.zone = "Bad Zone".into();
        dns.records[0].name = "-x".into();
        dns.dhcp.as_mut().unwrap().hosts[0].name = String::new();
        let found = errors(&dns);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].starts_with("dns.zone"));
        assert!(found[1].starts_with("dns.records[0].name"));
        assert!(found[2].starts_with("dns.dhcp.hosts[0].name"));
        assert!(valid_label("Retro-Net1") && valid_label("a") && !valid_label(&"a".repeat(64)));
        assert!(!valid_label("a-") && !valid_label("a.b") && !valid_label("a_b"));
    }

    #[test]
    fn addresses_must_be_ipv4_in_the_subnet_and_not_the_gateway() {
        let mut dns = good();
        dns.records = [
            "nope",
            "10.0.0.5",
            "172.16.120.1",
            "172.16.120.0",
            "172.16.120.255",
        ]
        .iter()
        .map(|a| Record {
            name: "x".into(),
            address: a.to_string(),
        })
        .collect();
        let found = errors(&dns);
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found[0].contains("not an ipv4 address"));
        assert!(found[1].contains("outside 172.16.120.0/24"));
        assert!(found[2].contains("gateway"));
        assert!(found[3].contains("broadcast") && found[4].contains("broadcast"));
    }

    #[test]
    fn an_address_is_fine_inside_any_network() {
        let second = (
            Subnet::new(Ipv4Addr::new(198, 51, 100, 1), 24),
            Ipv4Addr::new(198, 51, 100, 1),
        );
        let nets = [(subnet(), GATEWAY), second];
        let mut dns = good();
        dns.records = ["172.16.120.9", "198.51.100.9", "203.0.113.9", "198.51.100.1"]
            .iter()
            .map(|a| Record {
                name: "x".into(),
                address: a.to_string(),
            })
            .collect();
        let found: Vec<String> = dns.validate(&nets).iter().map(ToString::to_string).collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("203.0.113.9 is outside every network"));
        assert!(found[1].contains("gateway"));
    }

    #[test]
    fn a_slash_31_has_no_reserved_addresses() {
        let tiny = Subnet::new(Ipv4Addr::new(10, 0, 0, 0), 31);
        let dns = DnsSettings {
            records: vec![Record {
                name: "x".into(),
                address: "10.0.0.1".into(),
            }],
            ..DnsSettings::default()
        };
        assert!(dns.validate(&[(tiny, Ipv4Addr::new(10, 0, 0, 0))]).is_empty());
    }

    #[test]
    fn the_range_must_be_ordered_and_inside() {
        let mut dns = good();
        let dhcp = dns.dhcp.as_mut().unwrap();
        dhcp.range_start = "172.16.120.200".into();
        dhcp.range_end = "172.16.120.100".into();
        assert!(errors(&dns)[0].starts_with("dns.dhcp.range:"));
        let dhcp = dns.dhcp.as_mut().unwrap();
        dhcp.range_start = "172.16.120.100".into();
        dhcp.range_end = "172.16.121.5".into();
        assert!(errors(&dns)[0].starts_with("dns.dhcp.range[1]"));
        dns.dhcp.as_mut().unwrap().range_end = "172.16.120.100".into();
        assert!(errors(&dns).is_empty(), "a one address range is fine");
    }

    #[test]
    fn leases_follow_the_playbook() {
        for ok in ["30m", "12h", "7d", "infinite", "INFINITE", "99999d"] {
            assert!(valid_lease(ok), "{ok}");
        }
        for bad in ["", "7", "d", "7w", "123456d", "-1d", "1.5h", "infinit"] {
            assert!(!valid_lease(bad), "{bad}");
        }
        let mut dns = good();
        dns.dhcp.as_mut().unwrap().lease = "forever".into();
        assert!(errors(&dns)[0].starts_with("dns.dhcp.lease"));
    }

    #[test]
    fn static_leases_need_a_mac_and_unique_values() {
        for ok in ["aa:bb:cc:dd:ee:ff", "AA:BB:CC:DD:EE:00"] {
            assert!(valid_mac(ok), "{ok}");
        }
        for bad in [
            "",
            "aa:bb:cc:dd:ee",
            "aa-bb-cc-dd-ee-ff",
            "aa:bb:cc:dd:ee:fg",
            "a:bb:cc:dd:ee:ff",
        ] {
            assert!(!valid_mac(bad), "{bad}");
        }
        let mut dns = good();
        let dhcp = dns.dhcp.as_mut().unwrap();
        dhcp.hosts.push(StaticLease {
            name: "other".into(),
            mac: "AA:BB:CC:DD:EE:FF".into(),
            address: "172.16.120.10".into(),
        });
        dhcp.hosts.push(StaticLease {
            name: "third".into(),
            mac: "bad".into(),
            address: "172.16.120.11".into(),
        });
        let found = errors(&dns);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].starts_with("dns.dhcp.hosts[1].mac") && found[0].contains("twice"));
        assert!(found[1].starts_with("dns.dhcp.hosts[1].address") && found[1].contains("twice"));
        assert!(found[2].starts_with("dns.dhcp.hosts[2].mac"));
    }

    const YAML: &str = "\
# my gateway
gateway:
  motd: hi
dns:
  zone: lab
  extra: keep-me
  records:
    - {name: ntsrv1, address: 172.16.120.10}
  dhcp:
    range: [172.16.120.100, 172.16.120.200]
    lease: 12h
    future: also-kept
    hosts:
      - {name: ntsrv1, mac: \"aa:bb:cc:dd:ee:ff\", address: 172.16.120.10}
nt4-domain:
  name: RETRONET
";

    #[test]
    fn reads_the_dns_section() {
        let dns = DnsSettings::from_settings(&Settings::parse(YAML).unwrap()).unwrap();
        assert_eq!(dns.zone, "lab");
        assert_eq!(dns.records[0].address, "172.16.120.10");
        let dhcp = dns.dhcp.unwrap();
        assert_eq!(
            (dhcp.range_start.as_str(), dhcp.lease.as_str()),
            ("172.16.120.100", "12h")
        );
        assert_eq!(dhcp.hosts[0].mac, "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn defaults_apply_when_parts_are_missing() {
        let none = DnsSettings::from_settings(&Settings::empty()).unwrap();
        assert_eq!(none, DnsSettings::default());
        assert_eq!(none.zone, "retronet");
        for text in ["dns: {}", "dns:", "dns: ~"] {
            let dns = DnsSettings::from_settings(&Settings::parse(text).unwrap()).unwrap();
            assert_eq!(dns, DnsSettings::default(), "{text}");
        }
        let dns = DnsSettings::from_settings(
            &Settings::parse("dns:\n  dhcp:\n    range: [10.0.0.2, 10.0.0.9]\n").unwrap(),
        )
        .unwrap();
        assert_eq!(dns.dhcp.unwrap().lease, "7d");
        assert_eq!(Dhcp::default().lease, "7d");
    }

    #[test]
    fn odd_shapes_are_errors_with_a_path() {
        for (text, wanted) in [
            ("dns: 3", "dns must be a mapping"),
            ("dns:\n  records: x", "dns.records is not a list"),
            ("dns:\n  records: [1]", "dns.records[0] is not a mapping"),
            ("dns:\n  records: [{name: a}]", "dns.records.address"),
            ("dns:\n  dhcp: 3", "dns.dhcp must be a mapping"),
            ("dns:\n  dhcp: {}", "dns.dhcp.range needs two"),
            ("dns:\n  dhcp: {range: [1.1.1.1, [2]]}", "dns.dhcp.range[1]"),
            (
                "dns:\n  dhcp: {range: [1.1.1.1, 1.1.1.2], hosts: [{name: a}]}",
                "dns.dhcp.hosts.mac",
            ),
        ] {
            let err = DnsSettings::from_settings(&Settings::parse(text).unwrap()).unwrap_err();
            assert!(err.to_string().contains(wanted), "{text}: {err}");
        }
    }

    #[test]
    fn a_round_trip_changes_nothing_but_dns() {
        let mut settings = Settings::parse(YAML).unwrap();
        let before = settings.clone();
        let mut dns = DnsSettings::from_settings(&settings).unwrap();
        dns.zone = "newzone".into();
        dns.records.push(Record {
            name: "second".into(),
            address: "172.16.120.11".into(),
        });
        dns.apply_to(&mut settings);
        for key in ["gateway", "nt4-domain"] {
            assert_eq!(settings.section(key), before.section(key), "{key}");
        }
        let again = DnsSettings::from_settings(&settings).unwrap();
        assert_eq!(again, dns);
        let text = settings.text();
        assert!(text.contains("extra: keep-me"), "{text}");
        assert!(text.contains("future: also-kept"), "{text}");
        assert!(
            !text.contains("# my gateway"),
            "typed edits re-dump the yaml"
        );
        let reparsed = Settings::parse(&text).unwrap();
        assert_eq!(DnsSettings::from_settings(&reparsed).unwrap(), dns);
    }

    #[test]
    fn removing_dhcp_and_writing_into_empty_settings() {
        let mut settings = Settings::parse(YAML).unwrap();
        let mut dns = DnsSettings::from_settings(&settings).unwrap();
        dns.dhcp = None;
        dns.apply_to(&mut settings);
        assert!(DnsSettings::from_settings(&settings)
            .unwrap()
            .dhcp
            .is_none());

        let mut fresh = Settings::empty();
        good().apply_to(&mut fresh);
        assert_eq!(DnsSettings::from_settings(&fresh).unwrap(), good());
        let mut bad_dns = Settings::parse("dns: 7\nmotd: x\n").unwrap();
        DnsSettings::default().apply_to(&mut bad_dns);
        assert!(bad_dns.text().contains("motd: x"));
    }

    #[test]
    fn numbers_read_as_text() {
        let settings =
            Settings::parse("dns:\n  zone: 5\n  dhcp: {range: [1.1.1.1, 1.1.1.2], lease: 30}\n")
                .unwrap();
        let dns = DnsSettings::from_settings(&settings).unwrap();
        assert_eq!(dns.zone, "5");
        assert_eq!(dns.dhcp.unwrap().lease, "30");
    }

    #[test]
    fn parsing_keeps_text_and_rejects_non_mappings() {
        let text = "# hello\nmotd: x # trailing\n";
        assert_eq!(Settings::parse(text).unwrap().text(), text);
        assert_eq!(
            Settings::parse("").unwrap(),
            Settings {
                root: Mapping::new(),
                raw: Some(String::new())
            }
        );
        assert_eq!(Settings::empty().text().trim(), "{}");
        assert_eq!(
            Settings::parse("# only a comment\n").unwrap().text(),
            "# only a comment\n"
        );
        assert!(Settings::parse("- a\n- b\n").is_err());
        assert!(Settings::parse("a: [unclosed\n").is_err());
        assert!(Settings::parse("x").is_err());
    }

    #[test]
    fn subnets_mask_their_host_bits() {
        let s = Subnet::new(Ipv4Addr::new(192, 168, 7, 130), 25);
        assert_eq!(s.network, Ipv4Addr::new(192, 168, 7, 128));
        assert_eq!(s.broadcast(), Ipv4Addr::new(192, 168, 7, 255));
        assert!(s.contains(Ipv4Addr::new(192, 168, 7, 200)));
        assert!(!s.contains(Ipv4Addr::new(192, 168, 7, 5)));
        assert_eq!(s.to_string(), "192.168.7.128/25");
        let all = Subnet::new(Ipv4Addr::new(1, 2, 3, 4), 0);
        assert!(all.contains(Ipv4Addr::new(200, 1, 1, 1)));
        let one = Subnet::new(Ipv4Addr::new(1, 2, 3, 4), 99);
        assert_eq!(one.prefix, 32);
        assert!(
            one.contains(Ipv4Addr::new(1, 2, 3, 4)) && !one.contains(Ipv4Addr::new(1, 2, 3, 5))
        );
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;

    fn yaml(text: &str) -> Value {
        parse_value(text.as_bytes()).unwrap()
    }

    const FULL: &str = "\
dns: {zone: lab}
nt4-domain:
  name: RETRONET
  admin_password: hunter2
  users:
    - name: dana
      password: pw1
      shell: sh
    - name: olive
web-proxy:
  token: t0k
";

    #[test]
    fn splitting_moves_only_secret_scalars() {
        let (plain, secrets) = split(&yaml(FULL));
        assert!(secret_paths(&plain).is_empty());
        assert_eq!(
            secret_paths(&secrets),
            [
                "nt4-domain.admin_password",
                "nt4-domain.users[dana].password",
                "web-proxy.token"
            ]
        );
        assert_eq!(plain.get("dns"), yaml(FULL).get("dns"));
        let users = &plain["nt4-domain"]["users"];
        assert_eq!(users[0]["shell"], Value::String("sh".into()));
        assert!(users[0].get("password").is_none());
        // the secrets list is tagged with names so it can be matched again
        assert_eq!(
            secrets["nt4-domain"]["users"][0]["name"],
            Value::String("dana".into())
        );
    }

    #[test]
    fn merging_undoes_splitting() {
        let full = yaml(FULL);
        let (plain, secrets) = split(&full);
        assert_eq!(merge(&plain, &secrets), full);
        assert_eq!(merge(&plain, &Value::Null), plain);
    }

    #[test]
    fn merging_matches_users_by_name_and_drops_orphans() {
        let plain = yaml("users: [{name: a}, {name: b}]\n");
        let secrets = yaml("users: [{name: b, password: x}, {name: gone, password: y}]\n");
        let merged = merge(&plain, &secrets);
        assert_eq!(merged["users"][1]["password"], Value::String("x".into()));
        assert_eq!(merged["users"].as_sequence().unwrap().len(), 2);
        // without names, positions line up
        let merged = merge(
            &yaml("l: [{a: 1}]\n"),
            &yaml("l: [{password: z}, {password: q}]\n"),
        );
        assert_eq!(merged["l"][0]["password"], Value::String("z".into()));
        assert_eq!(merged["l"].as_sequence().unwrap().len(), 1);
        // a secret overrides a plain scalar and fills a missing section
        let merged = merge(
            &yaml("a: {password: old}\n"),
            &yaml("a: {password: new}\nb: {token: t}\n"),
        );
        assert_eq!(merged["a"]["password"], Value::String("new".into()));
        assert_eq!(merged["b"]["token"], Value::String("t".into()));
    }

    #[test]
    fn unnamed_list_entries_keep_their_secrets_in_the_plain_part() {
        let (plain, secrets) = split(&yaml("l: [{password: z}]\n"));
        assert_eq!(secret_paths(&plain), ["l[0].password"]);
        assert!(secret_paths(&secrets).is_empty());
    }

    #[test]
    fn secret_fields_list_what_the_settings_call_for() {
        let plain = "nt4-domain:\n  users: [{name: dana}, {name: olive}]\nother:\n  users: [{name: x}]\n  k: 1\nnum: 3\n";
        let fields = secret_fields(
            plain,
            &[
                "nt4-domain.users[dana].password".to_string(),
                "web-proxy.token".to_string(),
            ],
        );
        let summary: Vec<(&str, bool)> = fields.iter().map(|f| (f.path.as_str(), f.set)).collect();
        assert_eq!(
            summary,
            [
                ("nt4-domain.admin_password", false),
                ("nt4-domain.users[dana].password", true),
                ("nt4-domain.users[olive].password", false),
                ("other.users[x].password", false),
                ("web-proxy.token", true),
            ]
        );
        assert_eq!(
            secret_fields("a: [oops", &[]).len(),
            0,
            "bad yaml implies nothing"
        );
        let doc = Document {
            plain: "nt4-domain: {}\n".into(),
            ..Document::default()
        };
        assert_eq!(doc.secret_fields().len(), 1);
    }

    #[test]
    fn removing_secrets_drops_empty_entries() {
        let mut secrets = Value::Mapping(Mapping::new());
        set_secret(&mut secrets, "nt4-domain.admin_password", "a").unwrap();
        set_secret(&mut secrets, "nt4-domain.users[dana].password", "b").unwrap();
        set_secret(&mut secrets, "nt4-domain.users[olive].password", "d").unwrap();
        remove_secret(&mut secrets, "nt4-domain.users[dana].password").unwrap();
        remove_secret(&mut secrets, "nt4-domain.users[nobody].password").unwrap();
        remove_secret(&mut secrets, "other.users[dana].password").unwrap();
        remove_secret(&mut secrets, "nt4-domain.admin_password.password").unwrap();
        assert!(remove_secret(&mut secrets, "nt4-domain.name").is_err());
        assert_eq!(
            secret_paths(&secrets),
            [
                "nt4-domain.admin_password",
                "nt4-domain.users[olive].password"
            ]
        );
        assert_eq!(
            secrets["nt4-domain"]["users"].as_sequence().unwrap().len(),
            1
        );
        remove_secret(&mut secrets, "nt4-domain.admin_password").unwrap();
        assert_eq!(secret_paths(&secrets).len(), 1);
        remove_secret(&mut Value::Null, "a.password").unwrap();
        remove_secret(&mut Value::Sequence(vec![]), "a.password").unwrap();
        remove_secret(&mut Value::Mapping(Mapping::new()), "a[x].password").unwrap();
    }

    #[test]
    fn setting_secrets_builds_the_path() {
        let mut secrets = Value::Mapping(Mapping::new());
        set_secret(&mut secrets, "nt4-domain.admin_password", "a").unwrap();
        set_secret(&mut secrets, "nt4-domain.users[dana].password", "b").unwrap();
        set_secret(&mut secrets, "nt4-domain.users[dana].password", "c").unwrap();
        set_secret(&mut secrets, "nt4-domain.users[olive].password", "d").unwrap();
        assert_eq!(
            secret_paths(&secrets),
            [
                "nt4-domain.admin_password",
                "nt4-domain.users[dana].password",
                "nt4-domain.users[olive].password"
            ]
        );
        assert_eq!(
            secrets["nt4-domain"]["users"][0]["password"],
            Value::String("c".into())
        );
        assert_eq!(
            secrets["nt4-domain"]["users"].as_sequence().unwrap().len(),
            2
        );
    }

    #[test]
    fn only_secret_paths_can_be_set() {
        let mut secrets = Value::Mapping(Mapping::new());
        for bad in [
            "",
            "nt4-domain.name",
            "users[dana]",
            "a.users[].password",
            "a.b[c",
        ] {
            assert!(set_secret(&mut secrets, bad, "x").is_err(), "{bad:?}");
        }
        for ok in SECRET_KEYS {
            set_secret(&mut secrets, &format!("s.{ok}"), "x").unwrap();
        }
        // a path that runs through a stored value is refused, not written over it
        let mut odd = yaml("a: 1\n");
        assert!(set_secret(&mut odd, "a.password", "x").is_err());
        assert!(set_secret(&mut odd, "a[x].password", "x").is_err());
        assert_eq!(odd, yaml("a: 1\n"));
        // nothing stored there yet: the containers are created
        let mut fresh = yaml("a: ~\n");
        set_secret(&mut fresh, "a.password", "x").unwrap();
        assert_eq!(fresh["a"]["password"], Value::String("x".into()));
        let mut fresh = yaml("a: ~\n");
        set_secret(&mut fresh, "a[x].password", "x").unwrap();
        assert_eq!(fresh["a"][0]["name"], Value::String("x".into()));
    }

    #[test]
    fn a_path_that_does_not_fit_the_stored_value_is_refused_and_changes_nothing() {
        let stored =
            "docker:\n  token: keep\nnt4-domain:\n  users:\n    - name: dana\n      password: pw\n";
        let mut secrets = yaml(stored);
        let err = set_secret(&mut secrets, "docker[a].token", "x").unwrap_err();
        assert!(err.to_string().contains("docker[a].token"), "{err}");
        assert!(set_secret(&mut secrets, "nt4-domain.users.0.password", "x").is_err());
        assert!(set_secret(&mut secrets, "docker.token.password", "x").is_err());
        assert_eq!(secrets, yaml(stored));
        // valid paths still work next to what is stored
        set_secret(&mut secrets, "nt4-domain.users[dana].password", "new").unwrap();
        assert_eq!(
            secrets["nt4-domain"]["users"][0]["password"],
            Value::String("new".into())
        );
        assert_eq!(secrets["docker"]["token"], Value::String("keep".into()));
    }

    #[test]
    fn parsing_rejects_non_mappings() {
        assert!(parse_value(b"- a\n").is_err());
        assert!(parse_value(b"[oops").is_err());
        assert_eq!(parse_value(b"").unwrap(), Value::Mapping(Mapping::new()));
        assert!(text_of_bytes(vec![0xff, 0xfe]).is_err());
    }
}
