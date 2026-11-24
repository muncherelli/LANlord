//! the read-only iis 4 hardening checks, worked out from the key=value lines of `lldump.vbs`.
//! every check is a pure function of the dump: nothing here talks to a machine, and nothing is
//! changed. a property that may hold a secret never reaches here (the script prints only
//! `<redacted>` for it). `fix_available` says whether a later fix exists; none runs yet.

use std::collections::{BTreeMap, BTreeSet};

use lanlord_model::hosting::{CheckResult, CheckStatus};

/// the properties of one node of the dump, each with its values in order.
type Props = BTreeMap<String, Vec<String>>;

/// does the dump show a service?
#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Present,
    Absent,
    /// the dump says nothing about it.
    Unseen,
}

/// the dump as nodes (`W3SVC/1/ROOT`) with their properties. the lines of the machine itself
/// start with `Host/`: `Host/Folder/IISSamples=present` is node `Host/Folder`, property
/// `IISSamples`.
struct Dump {
    nodes: BTreeMap<String, Props>,
    absent: BTreeSet<String>,
}

impl Dump {
    fn parse(text: &str) -> Dump {
        let mut dump = Dump {
            nodes: BTreeMap::new(),
            absent: BTreeSet::new(),
        };
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once('=') else {
                continue;
            };
            let Some((node, property)) = key.rsplit_once('/') else {
                if value.trim() == "absent" {
                    dump.absent.insert(key.to_string());
                }
                continue;
            };
            let property = property.split('[').next().unwrap_or(property);
            let value: String = value.chars().filter(|c| !c.is_control()).collect();
            dump.nodes
                .entry(node.to_string())
                .or_default()
                .entry(property.to_string())
                .or_default()
                .push(value.trim().to_string());
        }
        dump
    }

    fn first(&self, node: &str, property: &str) -> Option<&str> {
        let values = self.nodes.get(node)?.get(property)?;
        values.first().map(String::as_str)
    }

    fn presence(&self, service: &str) -> Presence {
        if self.nodes.contains_key(service) {
            Presence::Present
        } else if self.absent.contains(service) {
            Presence::Absent
        } else {
            Presence::Unseen
        }
    }

    /// the nodes of a service, the service itself included.
    fn under<'a>(&'a self, service: &str) -> impl Iterator<Item = (&'a str, &'a Props)> + 'a {
        let service = service.to_string();
        self.nodes
            .iter()
            .filter(move |(node, _)| {
                node.as_str() == service
                    || node
                        .strip_prefix(service.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(node, props)| (node.as_str(), props))
    }

    /// the nodes of the sites of a service: `W3SVC/1`, `W3SVC/2`.
    fn sites(&self, service: &str) -> Vec<&str> {
        self.under(service)
            .map(|(node, _)| node)
            .filter(|node| {
                let rest = &node[service.len()..];
                rest.strip_prefix('/')
                    .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            })
            .collect()
    }

    /// the directories below `ROOT` of every site, as `site/ROOT/path`, with the path in lower case.
    fn vdirs(&self, service: &str) -> Vec<(&str, String)> {
        let mut found = Vec::new();
        for (node, props) in self.under(service) {
            if !props.contains_key("Class") {
                continue;
            }
            let mut parts = node.splitn(4, '/');
            let (_, _, root, rest) = (parts.next(), parts.next(), parts.next(), parts.next());
            if let (Some("ROOT"), Some(rest)) = (root, rest) {
                found.push((node, rest.to_ascii_lowercase()));
            }
        }
        found
    }

    /// the machine's own facts of one kind, as (name, value).
    fn host(&self, kind: &str) -> Vec<(&str, &str)> {
        let node = format!("Host/{kind}");
        self.nodes.get(&node).map_or_else(Vec::new, |props| {
            props
                .iter()
                .filter_map(|(name, v)| Some((name.as_str(), v.first()?.as_str())))
                .collect()
        })
    }
}

/// what a search found: the hits, whether there was anything to search, and whether some of what
/// it needed was missing or could not be told.
struct Finding {
    hits: Vec<String>,
    looked: bool,
    unsure: bool,
}

impl Finding {
    fn none() -> Finding {
        Finding {
            hits: Vec::new(),
            looked: false,
            unsure: false,
        }
    }

    /// two searches that must both have looked for the answer to be a pass.
    fn merge(mut self, other: Finding) -> Finding {
        self.hits.extend(other.hits);
        self.hits.sort();
        self.hits.dedup();
        self.looked &= other.looked;
        self.unsure |= other.unsure;
        self
    }
}

/// at most three names, then how many more.
fn few(items: &[String]) -> String {
    let shown: Vec<&str> = items.iter().take(3).map(String::as_str).collect();
    match items.len().saturating_sub(3) {
        0 => shown.join("; "),
        more => format!("{} and {more} more", shown.join("; ")),
    }
}

fn result(id: &str, status: CheckStatus, evidence: impl Into<String>, fix: bool) -> CheckResult {
    CheckResult {
        id: id.into(),
        status,
        evidence: evidence.into(),
        fix_available: fix,
    }
}

/// fail with the hits, pass when there were none, unknown when nothing was looked at.
fn finding(id: &str, found: Finding, (hit, clear): (&str, &str), fix: bool) -> CheckResult {
    if !found.hits.is_empty() {
        let names: Vec<String> = found.hits.iter().map(|h| format!("{h} {hit}")).collect();
        return result(id, CheckStatus::Fail, few(&names), fix);
    }
    if found.looked && !found.unsure {
        result(id, CheckStatus::Pass, clear, fix)
    } else {
        result(
            id,
            CheckStatus::Unknown,
            "the dump lacks some of the data for this",
            fix,
        )
    }
}

/// a boolean as the metabase prints it; anything else is not an answer.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "-1" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn truthy(value: &str) -> bool {
    parse_bool(value) == Some(true)
}

/// is any bit of `mask` set in a flags value? adsi returns flags with the top bit set as a
/// negative number, so the value is read as signed and cut to 32 bits.
fn bit(value: &str, mask: u32) -> Option<bool> {
    let number = value.trim().parse::<i64>().ok()?;
    Some((number as u32) & mask != 0)
}

/// the nodes of a service where `property` is set and `bad` says so; `looked` when any node has
/// it, and `unsure` when a node has a value `bad` cannot read (`None`).
fn scan(dump: &Dump, service: &str, property: &str, bad: impl Fn(&str) -> Option<bool>) -> Finding {
    let mut found = Finding::none();
    for (node, props) in dump.under(service) {
        if let Some(value) = props.get(property).and_then(|v| v.first()) {
            found.looked = true;
            match bad(value) {
                Some(true) => found.hits.push(node.to_string()),
                Some(false) => {}
                None => found.unsure = true,
            }
        }
    }
    found
}

/// the directories below `ROOT` of a service whose path is one of `names`.
fn vdir_hits(dump: &Dump, service: &str, names: &[&str]) -> Finding {
    Finding {
        hits: dump
            .vdirs(service)
            .into_iter()
            .filter(|(_, path)| names.contains(&path.as_str()))
            .map(|(node, _)| node.to_string())
            .collect(),
        looked: dump.presence(service) == Presence::Present,
        unsure: false,
    }
}

/// the machine's own facts of one kind that say `present`, for the names in `wanted`. a name
/// with no line, or one that says `unknown`, leaves the search unsure; with no line at all
/// nothing was looked at.
fn host_hits(dump: &Dump, kind: &str, wanted: &[&str]) -> Finding {
    let all = dump.host(kind);
    let state = |name: &str| all.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    let known = |name: &&str| matches!(state(name), Some("present" | "absent"));
    Finding {
        hits: wanted
            .iter()
            .filter(|name| state(name) == Some("present"))
            .map(|name| format!("{kind} {name}"))
            .collect(),
        looked: wanted.iter().any(known),
        unsure: !wanted.iter().all(known),
    }
}

fn samples(dump: &Dump) -> CheckResult {
    let mut found = vdir_hits(dump, "W3SVC", &["iissamples", "exair"]);
    found = found.merge(host_hits(dump, "Folder", &["IISSamples"]));
    found = found.merge(host_hits(dump, "File", &["codebrws.asp"]));
    finding(
        "iis.samples",
        found,
        ("present", "no sample vdirs, folders or files"),
        true,
    )
}

fn tools(dump: &Dump) -> CheckResult {
    let mut found = vdir_hits(dump, "W3SVC", &["scripts/tools"]);
    found = found.merge(host_hits(dump, "Folder", &["ScriptsTools"]));
    found = found.merge(host_hits(dump, "File", &["showcode.asp"]));
    finding(
        "iis.tools",
        found,
        ("present", "no scripts/tools directory or showcode file"),
        true,
    )
}

fn msadc(dump: &Dump) -> CheckResult {
    let found = vdir_hits(dump, "W3SVC", &["msadc"]).merge(host_hits(
        dump,
        "Reg/ADCLaunch",
        &[
            "RDSServer.DataFactory",
            "AdvancedDataFactory",
            "VbBusObj.VbBusObjCls",
        ],
    ));
    finding(
        "iis.msadc",
        found,
        ("present", "no MSADC vdir and no ADCLaunch keys"),
        true,
    )
}

fn vdir_check(dump: &Dump, id: &str, name: &str) -> CheckResult {
    let found = vdir_hits(dump, "W3SVC", &[name]);
    finding(id, found, ("present", &format!("no {name} vdir")), true)
}

/// the extensions that map to something IIS 4 was attacked through.
const RISKY_MAPS: [&str; 9] = [
    ".htr", ".idc", ".ida", ".idq", ".stm", ".shtm", ".shtml", ".cer", ".cdx",
];

fn scriptmaps(dump: &Dump) -> CheckResult {
    let mut seen = BTreeSet::new();
    let mut first = None;
    let mut looked = false;
    for (node, props) in dump.under("W3SVC") {
        let Some(maps) = props.get("ScriptMaps") else {
            continue;
        };
        looked = true;
        for map in maps {
            let ext = map.split(',').next().unwrap_or("").to_ascii_lowercase();
            if let Some(risky) = RISKY_MAPS.iter().find(|r| **r == ext) {
                seen.insert(*risky);
                first.get_or_insert(node);
            }
        }
    }
    match (first, looked) {
        (Some(node), _) => {
            let list: Vec<&str> = seen.into_iter().collect();
            result(
                "iis.scriptmaps",
                CheckStatus::Fail,
                format!("{} mapped, first at {node}", list.join(" ")),
                true,
            )
        }
        (None, true) => result(
            "iis.scriptmaps",
            CheckStatus::Pass,
            "none of the risky extensions is mapped",
            true,
        ),
        (None, false) => result(
            "iis.scriptmaps",
            CheckStatus::Unknown,
            "the dump has no script maps",
            true,
        ),
    }
}

fn flag(
    dump: &Dump,
    (id, property): (&str, &str),
    what: &str,
    bad: fn(&str) -> Option<bool>,
) -> CheckResult {
    let found = scan(dump, "W3SVC", property, bad);
    finding(
        id,
        found,
        (what, &format!("{property} is not risky anywhere")),
        true,
    )
}

fn dir_browsing(dump: &Dump) -> CheckResult {
    // either the flag or the browse bit (0x80000000) on says the directory is listed
    let flag = scan(dump, "W3SVC", "EnableDirBrowsing", parse_bool);
    let bits = scan(dump, "W3SVC", "DirBrowseFlags", |v| bit(v, 0x8000_0000));
    let mut found = Finding {
        hits: flag.hits,
        looked: flag.looked || bits.looked,
        unsure: flag.unsure || bits.unsure,
    };
    found.hits.extend(bits.hits);
    found.hits.sort();
    found.hits.dedup();
    finding(
        "iis.dir-browsing",
        found,
        ("lists directories", "directory browsing is off everywhere"),
        true,
    )
}

fn write_access(dump: &Dump) -> CheckResult {
    let found = scan(dump, "W3SVC", "AccessFlags", |v| bit(v, 2));
    finding(
        "iis.write",
        found,
        ("allows write", "no web directory allows write"),
        true,
    )
}

fn admin_site(dump: &Dump) -> CheckResult {
    let sites = dump.sites("W3SVC");
    let mut open = Vec::new();
    let mut found = 0;
    for node in &sites {
        let comment = dump.first(node, "ServerComment").unwrap_or("");
        if !comment.eq_ignore_ascii_case("Administration Web Site") {
            continue;
        }
        found += 1;
        let started = dump.first(node, "ServerState") == Some("2");
        let auto = dump.first(node, "ServerAutoStart").is_some_and(truthy);
        if started || auto {
            open.push(format!("{node} runs or starts with the service"));
        }
    }
    match (open.is_empty(), found, sites.is_empty()) {
        (false, ..) => result("iis.admin-site", CheckStatus::Fail, few(&open), true),
        (true, 0, true) => result(
            "iis.admin-site",
            CheckStatus::Unknown,
            "the dump has no web sites",
            true,
        ),
        (true, 0, false) => result(
            "iis.admin-site",
            CheckStatus::Pass,
            "no site is named Administration Web Site",
            true,
        ),
        (true, _, _) => result(
            "iis.admin-site",
            CheckStatus::Pass,
            "the administration site is stopped and does not autostart",
            true,
        ),
    }
}

fn anon_account(dump: &Dump) -> CheckResult {
    let mut names = BTreeSet::new();
    for (_, props) in dump.under("W3SVC") {
        if let Some(name) = props.get("AnonymousUserName").and_then(|v| v.first()) {
            names.insert(name.clone());
        }
    }
    let Some(odd) = names
        .iter()
        .find(|n| n.to_ascii_lowercase().contains("admin"))
    else {
        let custom: Vec<String> = names
            .iter()
            .filter(|n| !n.to_ascii_lowercase().starts_with("iusr_"))
            .cloned()
            .collect();
        return match (names.is_empty(), custom.is_empty()) {
            (true, _) => result(
                "iis.anon-account",
                CheckStatus::Unknown,
                "the dump has no anonymous account",
                true,
            ),
            (false, true) => result(
                "iis.anon-account",
                CheckStatus::Pass,
                format!(
                    "anonymous account {}",
                    few(&names.iter().cloned().collect::<Vec<_>>())
                ),
                true,
            ),
            (false, false) => result(
                "iis.anon-account",
                CheckStatus::Warn,
                format!("not an IUSR_ account: {}", few(&custom)),
                true,
            ),
        };
    };
    result(
        "iis.anon-account",
        CheckStatus::Fail,
        format!("anonymous account {odd} looks like an administrator"),
        true,
    )
}

/// does a site have a secure (ssl) binding?
fn has_tls(dump: &Dump, site: &str) -> bool {
    dump.nodes
        .get(site)
        .and_then(|props| props.get("SecureBindings"))
        .is_some_and(|b| b.iter().any(|v| !v.trim().is_empty()))
}

fn basic_no_tls(dump: &Dump) -> CheckResult {
    let mut looked = false;
    let mut hits = Vec::new();
    for site in dump.sites("W3SVC") {
        let flags: Vec<&String> = dump
            .under(site)
            .filter_map(|(_, props)| props.get("AuthFlags")?.first())
            .collect();
        looked |= !flags.is_empty();
        if flags.iter().any(|v| bit(v, 2) == Some(true)) && !has_tls(dump, site) {
            hits.push(format!("{site} allows Basic and has no TLS binding"));
        }
    }
    match (hits.is_empty(), looked) {
        (false, _) => result("iis.basic-no-tls", CheckStatus::Warn, few(&hits), false),
        (true, true) => result(
            "iis.basic-no-tls",
            CheckStatus::Pass,
            "no site allows Basic without TLS",
            false,
        ),
        (true, false) => result(
            "iis.basic-no-tls",
            CheckStatus::Unknown,
            "the dump has no authentication flags",
            false,
        ),
    }
}

/// the class id of the W3C extended log format plug-in.
const W3C_LOG: &str = "{ff160663-de82-11cf-bc0a-00aa006111e0}";

fn logging(dump: &Dump) -> CheckResult {
    let mut looked = false;
    let (mut off, mut other) = (Vec::new(), Vec::new());
    for site in dump.sites("W3SVC") {
        let Some(kind) = dump.first(site, "LogType") else {
            continue;
        };
        looked = true;
        let plugin = dump.first(site, "LogPluginClsid").unwrap_or("");
        if kind.trim() == "0" {
            off.push(format!("{site} logs nothing"));
        } else if !plugin.eq_ignore_ascii_case(W3C_LOG) {
            other.push(format!("{site} does not log in W3C extended format"));
        }
    }
    if !off.is_empty() {
        return result("iis.logging", CheckStatus::Fail, few(&off), true);
    }
    if !other.is_empty() {
        return result("iis.logging", CheckStatus::Warn, few(&other), true);
    }
    if looked {
        result(
            "iis.logging",
            CheckStatus::Pass,
            "W3C extended logging is on",
            true,
        )
    } else {
        result(
            "iis.logging",
            CheckStatus::Unknown,
            "the dump has no log settings",
            true,
        )
    }
}

fn ftp_anon_write(dump: &Dump) -> CheckResult {
    let id = "ftp.anon-write";
    let sites = dump.sites("MSFTPSVC");
    let mut hits = Vec::new();
    for site in &sites {
        if !dump.first(site, "AllowAnonymous").is_some_and(truthy) {
            continue;
        }
        hits.extend(
            dump.under(site)
                .filter(|(_, props)| {
                    props
                        .get("AccessFlags")
                        .and_then(|v| v.first())
                        .is_some_and(|v| bit(v, 2) == Some(true))
                })
                .map(|(node, _)| format!("{node} lets anonymous users write")),
        );
    }
    match (hits.is_empty(), sites.is_empty(), dump.presence("MSFTPSVC")) {
        (false, ..) => result(id, CheckStatus::Fail, few(&hits), true),
        (true, false, _) => result(id, CheckStatus::Pass, "no anonymous FTP write", true),
        (true, true, Presence::Unseen) => {
            result(id, CheckStatus::Unknown, "the dump has no FTP data", true)
        }
        (true, true, _) => result(id, CheckStatus::NotApplicable, "no FTP site", true),
    }
}

fn urlscan(dump: &Dump) -> CheckResult {
    let id = "iis.urlscan";
    if dump.presence("W3SVC") != Presence::Present {
        return result(
            id,
            CheckStatus::Unknown,
            "the dump has no web service",
            true,
        );
    }
    let present: Vec<&str> = dump
        .under("W3SVC")
        .filter_map(|(node, props)| {
            let path = props.get("FilterPath")?.first()?;
            path.to_ascii_lowercase()
                .contains("urlscan")
                .then_some(node)
        })
        .collect();
    match present.first() {
        Some(node) => result(id, CheckStatus::Pass, format!("{node} is installed"), true),
        None => result(
            id,
            CheckStatus::Warn,
            "no URLScan filter in the filter list",
            true,
        ),
    }
}

fn frontpage(dump: &Dump) -> CheckResult {
    let mut found = vdir_hits(dump, "W3SVC", &["_vti_bin", "_vti_adm", "_vti_aut"]);
    found = found.merge(host_hits(dump, "Folder", &["FPSE"]));
    if found.hits.is_empty() {
        return finding(
            "iis.frontpage",
            found,
            ("", "no FrontPage extensions"),
            false,
        );
    }
    let names: Vec<String> = found.hits.iter().map(|h| format!("{h} present")).collect();
    result("iis.frontpage", CheckStatus::Warn, few(&names), false)
}

fn patch(dump: &Dump) -> CheckResult {
    let id = "iis.patch";
    let hotfix = dump.host("Reg/Hotfix");
    match hotfix.iter().find(|(name, _)| *name == "Q811114") {
        Some((_, "present")) => result(
            id,
            CheckStatus::Pass,
            "Q811114 (MS03-018) is installed",
            false,
        ),
        Some(_) => result(
            id,
            CheckStatus::Fail,
            "Q811114 (MS03-018) is not installed; LANlord installs no patches",
            false,
        ),
        None => result(
            id,
            CheckStatus::Unknown,
            "the dump has no hotfix data",
            false,
        ),
    }
}

fn services_unused(dump: &Dump) -> CheckResult {
    let id = "services.unused";
    let services = dump.nodes.keys().filter(|n| n.starts_with("Host/Svc/"));
    let (mut looked, mut auto) = (false, Vec::new());
    for node in services {
        let start = dump.first(node, "Start").unwrap_or("");
        looked = true;
        if start == "2" {
            auto.push(format!(
                "{} starts automatically",
                &node["Host/Svc/".len()..]
            ));
        }
    }
    match (auto.is_empty(), looked) {
        (false, _) => result(id, CheckStatus::Warn, few(&auto), true),
        (true, true) => result(
            id,
            CheckStatus::Pass,
            "no unneeded service starts automatically",
            true,
        ),
        (true, false) => result(
            id,
            CheckStatus::Unknown,
            "the dump has no service data",
            true,
        ),
    }
}

/// the checks that read the web service; they are not applicable when it is not installed.
const WEB_CHECKS: [&str; 18] = [
    "iis.samples",
    "iis.iishelp",
    "iis.admin-site",
    "iis.iisadmpwd",
    "iis.msadc",
    "iis.tools",
    "iis.scriptmaps",
    "iis.isolation",
    "iis.parent-paths",
    "iis.errors-to-client",
    "iis.dir-browsing",
    "iis.write",
    "iis.anon-account",
    "iis.basic-no-tls",
    "iis.logging",
    "iis.urlscan",
    "iis.frontpage",
    "iis.printers",
];

/// every check, in a stable order, from the lines of `lldump.vbs`.
pub fn evaluate(text: &str) -> Vec<CheckResult> {
    let dump = Dump::parse(text);
    let mut checks = vec![
        samples(&dump),
        vdir_check(&dump, "iis.iishelp", "iishelp"),
        admin_site(&dump),
        vdir_check(&dump, "iis.iisadmpwd", "iisadmpwd"),
        msadc(&dump),
        tools(&dump),
        scriptmaps(&dump),
        flag(
            &dump,
            ("iis.isolation", "AppIsolated"),
            "runs ASP in the web server process",
            |v| v.trim().parse::<i64>().ok().map(|n| n == 0),
        ),
        flag(
            &dump,
            ("iis.parent-paths", "AspEnableParentPaths"),
            "allows parent paths",
            parse_bool,
        ),
        flag(
            &dump,
            ("iis.errors-to-client", "AspScriptErrorSentToBrowser"),
            "sends script errors to the browser",
            parse_bool,
        ),
        dir_browsing(&dump),
        write_access(&dump),
        anon_account(&dump),
        basic_no_tls(&dump),
        logging(&dump),
        urlscan(&dump),
        frontpage(&dump),
        result(
            "iis.printers",
            CheckStatus::NotApplicable,
            "/Printers and .printer exist on IIS 5 only",
            false,
        ),
    ];
    if dump.presence("W3SVC") == Presence::Absent {
        for check in checks
            .iter_mut()
            .filter(|c| WEB_CHECKS.contains(&c.id.as_str()))
        {
            check.status = CheckStatus::NotApplicable;
            check.evidence = "W3SVC is not installed".into();
        }
    }
    checks.push(ftp_anon_write(&dump));
    checks.push(patch(&dump));
    checks.push(result(
        "smtp.relay",
        CheckStatus::Unknown,
        "not run: needs a relay probe from the gateway",
        false,
    ));
    checks.push(services_unused(&dump));
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a badly configured WEB1 with an FTP site.
    const BAD: &str = "\
W3SVC/Class=IIsWebService
W3SVC/AnonymousUserName=IUSR_WEB1
W3SVC/AnonymousUserPass=<redacted>
W3SVC/ScriptMaps[0]=.asp,C:\\WINNT\\System32\\inetsrv\\asp.dll,5,GET,POST
W3SVC/ScriptMaps[1]=.htr,C:\\WINNT\\System32\\inetsrv\\ism.dll,5,GET,POST
W3SVC/ScriptMaps[2]=.IDQ,C:\\WINNT\\System32\\idq.dll,5,GET,POST
W3SVC/Filters/Class=IIsFilters
W3SVC/Filters/Compression/Class=IIsFilter
W3SVC/Filters/Compression/FilterPath=C:\\WINNT\\System32\\inetsrv\\gzip.dll
W3SVC/1/Class=IIsWebServer
W3SVC/1/ServerComment=Intranet
W3SVC/1/ServerState=2
W3SVC/1/LogType=1
W3SVC/1/LogPluginClsid={FF160663-DE82-11CF-BC0A-00AA006111E0}
W3SVC/1/AuthFlags=3
W3SVC/1/ROOT/Class=IIsWebVirtualDir
W3SVC/1/ROOT/AccessFlags=515
W3SVC/1/ROOT/AppIsolated=0
W3SVC/1/ROOT/AspEnableParentPaths=True
W3SVC/1/ROOT/AspScriptErrorSentToBrowser=True
W3SVC/1/ROOT/EnableDirBrowsing=True
W3SVC/1/ROOT/IISSamples/Class=IIsWebVirtualDir
W3SVC/1/ROOT/IISHelp/Class=IIsWebVirtualDir
W3SVC/1/ROOT/IISADMPWD/Class=IIsWebVirtualDir
W3SVC/1/ROOT/MSADC/Class=IIsWebVirtualDir
W3SVC/1/ROOT/scripts/Class=IIsWebVirtualDir
W3SVC/1/ROOT/scripts/tools/Class=IIsWebDirectory
W3SVC/1/ROOT/_vti_bin/Class=IIsWebVirtualDir
W3SVC/2/Class=IIsWebServer
W3SVC/2/ServerComment=Administration Web Site
W3SVC/2/ServerState=2
W3SVC/2/ServerAutoStart=True
MSFTPSVC/Class=IIsFtpService
MSFTPSVC/1/Class=IIsFtpServer
MSFTPSVC/1/AllowAnonymous=True
MSFTPSVC/1/ROOT/Class=IIsFtpVirtualDir
MSFTPSVC/1/ROOT/AccessFlags=3
SMTPSVC=absent
Host/Folder/IISSamples=present
Host/Folder/ScriptsTools=present
Host/Folder/FPSE=absent
Host/File/codebrws.asp=present
Host/File/showcode.asp=absent
Host/Reg/Hotfix/Q811114=absent
Host/Reg/ADCLaunch/RDSServer.DataFactory=present
Host/Reg/ADCLaunch/AdvancedDataFactory=absent
Host/Reg/ADCLaunch/VbBusObj.VbBusObjCls=unknown
Host/Svc/Alerter/Start=2
Host/Svc/Messenger/Start=4
Host/Svc/TlntSvr/Start=absent
";

    /// a tidy WEB1: nothing risky anywhere.
    const GOOD: &str = "\
W3SVC/Class=IIsWebService
W3SVC/AnonymousUserName=IUSR_WEB1
W3SVC/ScriptMaps[0]=.asp,C:\\WINNT\\System32\\inetsrv\\asp.dll,5,GET,POST
W3SVC/Filters/UrlScan/Class=IIsFilter
W3SVC/Filters/UrlScan/FilterPath=C:\\WINNT\\System32\\inetsrv\\urlscan\\urlscan.dll
W3SVC/1/Class=IIsWebServer
W3SVC/1/ServerComment=Intranet
W3SVC/1/ServerState=2
W3SVC/1/LogType=1
W3SVC/1/LogPluginClsid={ff160663-de82-11cf-bc0a-00aa006111e0}
W3SVC/1/AuthFlags=5
W3SVC/1/ROOT/Class=IIsWebVirtualDir
W3SVC/1/ROOT/AccessFlags=513
W3SVC/1/ROOT/AppIsolated=2
W3SVC/1/ROOT/AspEnableParentPaths=False
W3SVC/1/ROOT/AspScriptErrorSentToBrowser=False
W3SVC/1/ROOT/EnableDirBrowsing=False
W3SVC/1/ROOT/images/Class=IIsWebVirtualDir
MSFTPSVC=absent
SMTPSVC=absent
Host/Folder/IISSamples=absent
Host/Folder/ScriptsTools=absent
Host/Folder/FPSE=absent
Host/File/codebrws.asp=absent
Host/File/showcode.asp=absent
Host/Reg/Hotfix/Q811114=present
Host/Reg/ADCLaunch/RDSServer.DataFactory=absent
Host/Reg/ADCLaunch/AdvancedDataFactory=absent
Host/Reg/ADCLaunch/VbBusObj.VbBusObjCls=absent
Host/Svc/Alerter/Start=4
";

    fn get<'a>(checks: &'a [CheckResult], id: &str) -> &'a CheckResult {
        checks.iter().find(|c| c.id == id).expect(id)
    }

    fn status(checks: &[CheckResult], id: &str) -> CheckStatus {
        get(checks, id).status
    }

    #[test]
    fn a_badly_set_up_machine_fails_each_check() {
        let checks = evaluate(BAD);
        for id in [
            "iis.samples",
            "iis.iishelp",
            "iis.admin-site",
            "iis.iisadmpwd",
            "iis.msadc",
            "iis.tools",
            "iis.scriptmaps",
            "iis.isolation",
            "iis.parent-paths",
            "iis.errors-to-client",
            "iis.dir-browsing",
            "iis.write",
            "ftp.anon-write",
            "iis.patch",
        ] {
            assert_eq!(status(&checks, id), CheckStatus::Fail, "{id}");
        }
        for id in ["iis.urlscan", "services.unused", "iis.basic-no-tls"] {
            assert_eq!(status(&checks, id), CheckStatus::Warn, "{id}");
        }
        assert_eq!(status(&checks, "iis.anon-account"), CheckStatus::Pass);
        assert_eq!(status(&checks, "iis.logging"), CheckStatus::Pass);
        assert_eq!(status(&checks, "iis.frontpage"), CheckStatus::Warn);
    }

    #[test]
    fn the_evidence_names_the_site_and_the_thing() {
        let checks = evaluate(BAD);
        let samples = &get(&checks, "iis.samples").evidence;
        assert!(
            samples.contains("W3SVC/1/ROOT/IISSamples present"),
            "{samples}"
        );
        assert!(samples.contains("Folder IISSamples present"), "{samples}");
        assert_eq!(
            get(&checks, "iis.scriptmaps").evidence,
            ".htr .idq mapped, first at W3SVC"
        );
        assert!(get(&checks, "iis.tools")
            .evidence
            .contains("W3SVC/1/ROOT/scripts/tools present"));
        assert!(get(&checks, "iis.msadc")
            .evidence
            .contains("ADCLaunch RDSServer.DataFactory"));
        assert!(get(&checks, "iis.admin-site")
            .evidence
            .starts_with("W3SVC/2 "));
        assert!(get(&checks, "iis.basic-no-tls")
            .evidence
            .starts_with("W3SVC/1 "));
        assert_eq!(
            get(&checks, "ftp.anon-write").evidence,
            "MSFTPSVC/1/ROOT lets anonymous users write"
        );
        assert_eq!(
            get(&checks, "services.unused").evidence,
            "Alerter starts automatically"
        );
    }

    #[test]
    fn a_tidy_machine_passes() {
        let checks = evaluate(GOOD);
        for check in &checks {
            let want = match check.id.as_str() {
                "iis.printers" | "ftp.anon-write" => CheckStatus::NotApplicable,
                "smtp.relay" => CheckStatus::Unknown,
                _ => CheckStatus::Pass,
            };
            assert_eq!(check.status, want, "{check:?}");
        }
    }

    #[test]
    fn every_check_has_a_stable_id_in_a_stable_order() {
        let ids: Vec<String> = evaluate("").into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "iis.samples",
                "iis.iishelp",
                "iis.admin-site",
                "iis.iisadmpwd",
                "iis.msadc",
                "iis.tools",
                "iis.scriptmaps",
                "iis.isolation",
                "iis.parent-paths",
                "iis.errors-to-client",
                "iis.dir-browsing",
                "iis.write",
                "iis.anon-account",
                "iis.basic-no-tls",
                "iis.logging",
                "iis.urlscan",
                "iis.frontpage",
                "iis.printers",
                "ftp.anon-write",
                "iis.patch",
                "smtp.relay",
                "services.unused",
            ]
        );
    }

    #[test]
    fn a_dump_without_the_data_is_unknown_not_pass() {
        let checks = evaluate("");
        for check in &checks {
            if check.id == "iis.printers" {
                continue;
            }
            assert_eq!(check.status, CheckStatus::Unknown, "{check:?}");
        }
        // a web service with no sites and no host lines still says unknown for the host files
        let bare = evaluate("W3SVC/Class=IIsWebService\n");
        assert_eq!(status(&bare, "iis.patch"), CheckStatus::Unknown);
        assert_eq!(status(&bare, "iis.samples"), CheckStatus::Unknown);
        assert_eq!(status(&bare, "iis.write"), CheckStatus::Unknown);
    }

    #[test]
    fn a_missing_web_service_makes_the_web_checks_not_applicable() {
        let checks = evaluate("W3SVC=absent\nMSFTPSVC=absent\nSMTPSVC=absent\n");
        assert_eq!(status(&checks, "iis.samples"), CheckStatus::NotApplicable);
        assert_eq!(status(&checks, "iis.logging"), CheckStatus::NotApplicable);
        assert_eq!(get(&checks, "iis.write").evidence, "W3SVC is not installed");
        assert_eq!(
            status(&checks, "ftp.anon-write"),
            CheckStatus::NotApplicable
        );
        assert_eq!(status(&checks, "iis.patch"), CheckStatus::Unknown);
    }

    #[test]
    fn printers_are_never_applicable_on_iis_4() {
        for dump in [BAD, GOOD, ""] {
            let checks = evaluate(dump);
            assert_eq!(status(&checks, "iis.printers"), CheckStatus::NotApplicable);
            assert!(!get(&checks, "iis.printers").fix_available);
        }
    }

    #[test]
    fn smtp_relay_is_not_run_yet() {
        let check = get(&evaluate(GOOD), "smtp.relay").clone();
        assert_eq!(check.status, CheckStatus::Unknown);
        assert!(check.evidence.starts_with("not run"));
    }

    #[test]
    fn the_logging_and_account_rules() {
        let site = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n";
        let off = evaluate(&format!("{site}W3SVC/1/LogType=0\n"));
        assert_eq!(status(&off, "iis.logging"), CheckStatus::Fail);
        let ncsa = evaluate(&format!(
            "{site}W3SVC/1/LogType=1\nW3SVC/1/LogPluginClsid={{FF16065B-DE82-11CF-BC0A-00AA006111E0}}\n"
        ));
        assert_eq!(status(&ncsa, "iis.logging"), CheckStatus::Warn);
        let admin = evaluate(&format!("{site}W3SVC/1/AnonymousUserName=Administrator\n"));
        assert_eq!(status(&admin, "iis.anon-account"), CheckStatus::Fail);
        let custom = evaluate(&format!("{site}W3SVC/1/AnonymousUserName=webguest\n"));
        assert_eq!(status(&custom, "iis.anon-account"), CheckStatus::Warn);
    }

    #[test]
    fn basic_with_a_tls_binding_is_fine() {
        let dump = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n\
                    W3SVC/1/SecureBindings[0]=:443:\nW3SVC/1/AuthFlags=2\n";
        assert_eq!(
            status(&evaluate(dump), "iis.basic-no-tls"),
            CheckStatus::Pass
        );
    }

    #[test]
    fn directory_browsing_reads_the_flag_or_the_bits() {
        let base = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n";
        // adsi prints the browse bit (0x80000000) as a negative number
        let bits = evaluate(&format!("{base}W3SVC/1/ROOT/DirBrowseFlags=-2147483618\n"));
        assert_eq!(status(&bits, "iis.dir-browsing"), CheckStatus::Fail);
        // the load-default bit (0x40000000) is not browsing
        let default = evaluate(&format!("{base}W3SVC/1/ROOT/DirBrowseFlags=1073741854\n"));
        assert_eq!(status(&default, "iis.dir-browsing"), CheckStatus::Pass);
        // either one on is a fail
        let both = evaluate(&format!(
            "{base}W3SVC/1/ROOT/EnableDirBrowsing=False\nW3SVC/1/ROOT/DirBrowseFlags=-2147483618\n"
        ));
        assert_eq!(status(&both, "iis.dir-browsing"), CheckStatus::Fail);
        let off = evaluate(&format!("{base}W3SVC/1/ROOT/DirBrowseFlags=30\n"));
        assert_eq!(status(&off, "iis.dir-browsing"), CheckStatus::Pass);
    }

    #[test]
    fn a_value_set_at_the_service_or_below_is_found_either_way() {
        let tree = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n\
                    W3SVC/1/ROOT/Class=IIsWebVirtualDir\nW3SVC/1/ROOT/app/Class=IIsWebDirectory\n";
        // risky on the service, safe on the site below: the service default still fails
        let top = evaluate(&format!(
            "{tree}W3SVC/AspEnableParentPaths=True\nW3SVC/1/AspEnableParentPaths=False\n"
        ));
        let check = get(&top, "iis.parent-paths");
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.evidence, "W3SVC allows parent paths");
        // safe on the service, risky on one directory below
        let low = evaluate(&format!(
            "{tree}W3SVC/AspEnableParentPaths=False\nW3SVC/1/ROOT/app/AspEnableParentPaths=True\n"
        ));
        let check = get(&low, "iis.parent-paths");
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.evidence, "W3SVC/1/ROOT/app allows parent paths");
    }

    #[test]
    fn an_unreadable_boolean_is_unknown_not_off() {
        let tree = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n";
        for odd in ["maybe", "", "<object>"] {
            let dump =
                format!("{tree}W3SVC/AspEnableParentPaths={odd}\nW3SVC/EnableDirBrowsing={odd}\n");
            let checks = evaluate(&dump);
            assert_eq!(
                status(&checks, "iis.parent-paths"),
                CheckStatus::Unknown,
                "{odd}"
            );
            assert_eq!(
                status(&checks, "iis.dir-browsing"),
                CheckStatus::Unknown,
                "{odd}"
            );
        }
        for (text, want) in [("TRUE", true), ("-1", true), ("False", false), ("0", false)] {
            assert_eq!(parse_bool(text), Some(want), "{text}");
        }
        assert_eq!(parse_bool("2"), None);
        let flags = evaluate(&format!("{tree}W3SVC/1/ROOT/AccessFlags=lots\n"));
        assert_eq!(status(&flags, "iis.write"), CheckStatus::Unknown);
    }

    #[test]
    fn checks_that_need_the_machine_are_unknown_without_its_lines() {
        // the metabase alone cannot say whether the sample files are there
        let web = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n";
        let checks = evaluate(web);
        for id in ["iis.samples", "iis.tools", "iis.msadc", "iis.frontpage"] {
            assert_eq!(status(&checks, id), CheckStatus::Unknown, "{id}");
        }
        // an unknown line leaves the answer unknown, but a hit still fails
        let unsure = evaluate(&format!(
            "{web}Host/Folder/IISSamples=unknown\nHost/File/codebrws.asp=absent\n"
        ));
        assert_eq!(status(&unsure, "iis.samples"), CheckStatus::Unknown);
        let hit = evaluate(&format!(
            "{web}Host/Folder/IISSamples=present\nHost/File/codebrws.asp=unknown\n"
        ));
        assert_eq!(status(&hit, "iis.samples"), CheckStatus::Fail);
    }

    #[test]
    fn adc_launch_keys_that_could_not_be_read_never_pass() {
        let web = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n";
        let keys = |a: &str, b: &str, c: &str| {
            format!(
                "{web}Host/Reg/ADCLaunch/RDSServer.DataFactory={a}\n\
                 Host/Reg/ADCLaunch/AdvancedDataFactory={b}\n\
                 Host/Reg/ADCLaunch/VbBusObj.VbBusObjCls={c}\n"
            )
        };
        let unknown = evaluate(&keys("unknown", "unknown", "unknown"));
        assert_eq!(status(&unknown, "iis.msadc"), CheckStatus::Unknown);
        let clear = evaluate(&keys("absent", "absent", "absent"));
        assert_eq!(status(&clear, "iis.msadc"), CheckStatus::Pass);
        let found = evaluate(&keys("present", "unknown", "absent"));
        assert_eq!(status(&found, "iis.msadc"), CheckStatus::Fail);
    }

    #[test]
    fn a_common_files_path_that_could_not_be_found_is_unknown_not_absent() {
        // lldump prints unknown when CommonFilesDir is not in the registry
        let web = "W3SVC/Class=IIsWebService\nW3SVC/1/Class=IIsWebServer\n";
        let dump =
            format!("{web}Host/File/showcode.asp=unknown\nHost/Folder/ScriptsTools=absent\n");
        assert_eq!(status(&evaluate(&dump), "iis.tools"), CheckStatus::Unknown);
        let dump = format!("{web}Host/Folder/FPSE=unknown\n");
        assert_eq!(
            status(&evaluate(&dump), "iis.frontpage"),
            CheckStatus::Unknown
        );
        let dump = format!("{web}Host/Folder/FPSE=present\n");
        assert_eq!(status(&evaluate(&dump), "iis.frontpage"), CheckStatus::Warn);
    }

    #[test]
    fn no_secret_value_reaches_the_checks() {
        let json = serde_json::to_string(&evaluate(BAD)).unwrap();
        assert!(
            !json.contains("redacted") && !json.contains("Pass"),
            "{json}"
        );
    }

    #[test]
    fn long_hit_lists_are_cut() {
        let names: Vec<String> = (1..=5).map(|n| format!("W3SVC/{n}")).collect();
        assert_eq!(few(&names), "W3SVC/1; W3SVC/2; W3SVC/3 and 2 more");
    }
}
