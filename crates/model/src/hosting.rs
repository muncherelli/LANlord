//! what detection finds on an nt4 machine with iis: service state and the sites of the metabase,
//! read from `net start` and the key=value lines of `lldump.vbs`. only these facts are kept. a
//! property that may hold a secret is never stored, not even redacted: a site records only that it
//! has a password (`has_password`).

use serde::{Deserialize, Serialize};

/// the services detection looks at, as the service names and the text `net start` shows for them.
const SERVICES: [(&str, &str); 4] = [
    ("IISADMIN", "iis admin"),
    ("W3SVC", "world wide web"),
    ("MSFTPSVC", "ftp publishing"),
    ("SMTPSVC", "smtp"),
];

/// what detection found on one machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostingFacts {
    pub machine: String,
    /// seconds since the unix epoch.
    pub detected_at: u64,
    pub services: Vec<ServiceFact>,
    pub sites: Vec<SiteFact>,
    /// the hardening checks of the last detection, read-only; none for facts kept before checks
    /// existed.
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    /// what the server wants said about this detection (a damaged cache set aside); a reply only,
    /// never kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceFact {
    /// `W3SVC`, `MSFTPSVC`, `SMTPSVC` or `IISADMIN`.
    pub name: String,
    pub running: bool,
}

/// the kind of a site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    Web,
    Ftp,
    Smtp,
}

impl SiteKind {
    pub fn label(self) -> &'static str {
        match self {
            SiteKind::Web => "web",
            SiteKind::Ftp => "ftp",
            SiteKind::Smtp => "smtp",
        }
    }
}

/// one `ip:port:host` binding; an empty ip is every address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub ip: String,
    pub port: u16,
    pub host: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteFact {
    pub kind: SiteKind,
    pub id: u32,
    /// the server comment: the name of the site.
    pub comment: String,
    pub bindings: Vec<Binding>,
    /// the path of the root virtual directory, when it has one.
    pub home: String,
    /// `started`, `stopped` and so on, or `unknown`.
    pub state: String,
    /// some password property exists on the site or below it; its value is never kept.
    pub has_password: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
    NotApplicable,
    /// the dump lacks the data, or the check does not run yet.
    Unknown,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Fail => "fail",
            CheckStatus::Warn => "warn",
            CheckStatus::NotApplicable => "not-applicable",
            CheckStatus::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub id: String,
    pub status: CheckStatus,
    pub evidence: String,
    pub fix_available: bool,
}

impl CheckResult {
    /// [`line`](Self::line) cut to `width` characters, with an ellipsis where it was cut.
    pub fn line_fit(&self, width: usize) -> String {
        let line = self.line();
        if line.chars().count() <= width {
            return line;
        }
        let cut: String = line.chars().take(width.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }

    /// one line of text for a window: the status, the id and what was seen.
    pub fn line(&self) -> String {
        let status = match self.status {
            CheckStatus::NotApplicable => "N/A".to_string(),
            other => other.label().to_ascii_uppercase(),
        };
        format!("{status:<4} {}  {}", self.id, self.evidence)
    }
}

impl HostingFacts {
    /// how many checks failed and how many warn.
    pub fn check_counts(&self) -> (usize, usize) {
        let count = |status| self.checks.iter().filter(|c| c.status == status).count();
        (count(CheckStatus::Fail), count(CheckStatus::Warn))
    }

    /// is the iis admin service or any site there at all?
    pub fn iis_seen(&self) -> bool {
        !self.sites.is_empty() || self.services.iter().any(|s| s.running)
    }
}

/// the service states from the output of `net start`: a service is running when a line names it.
pub fn parse_services(net_start: &str) -> Vec<ServiceFact> {
    let lower = net_start.to_ascii_lowercase();
    SERVICES
        .iter()
        .map(|(name, text)| ServiceFact {
            name: (*name).into(),
            running: lower.lines().any(|line| line.contains(text)),
        })
        .collect()
}

/// `ip:port:host`, or `None` when it is not one.
fn parse_binding(text: &str) -> Option<Binding> {
    let mut parts = text.trim().splitn(3, ':');
    let ip = parts.next()?;
    let port = parts.next()?.trim().parse().ok()?;
    let host = parts.next().unwrap_or("");
    Some(Binding {
        ip: ip.into(),
        port,
        host: host.into(),
    })
}

fn state_name(code: &str) -> &'static str {
    match code.trim() {
        "1" => "starting",
        "2" => "started",
        "3" => "stopping",
        "4" => "stopped",
        "5" => "pausing",
        "6" => "paused",
        "7" => "continuing",
        _ => "unknown",
    }
}

/// the text without control characters: a name from a machine must not move a terminal.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .into()
}

/// the sites in the key=value lines of `lldump.vbs`. a line is `SERVICE/ID/Property=value`, with
/// `[n]` after the property for the items of a list; the root directory of a site is `ROOT`.
/// anything that is not a site line is ignored.
pub fn parse_dump(dump: &str) -> Vec<SiteFact> {
    let mut sites: Vec<SiteFact> = Vec::new();
    for line in dump.lines() {
        let Some((key, value)) = line.trim_end().split_once('=') else {
            continue;
        };
        let mut path = key.split('/');
        let kind = match path.next() {
            Some("W3SVC") => SiteKind::Web,
            Some("MSFTPSVC") => SiteKind::Ftp,
            Some("SMTPSVC") => SiteKind::Smtp,
            _ => continue,
        };
        let Some(id) = path.next().and_then(|id| id.parse::<u32>().ok()) else {
            continue;
        };
        let rest: Vec<&str> = path.collect();
        let at = match sites.iter().position(|s| s.kind == kind && s.id == id) {
            Some(at) => at,
            None => {
                sites.push(SiteFact {
                    kind,
                    id,
                    comment: String::new(),
                    bindings: Vec::new(),
                    home: String::new(),
                    state: "unknown".into(),
                    has_password: false,
                });
                sites.len() - 1
            }
        };
        let site = &mut sites[at];
        let Some(last) = rest.last() else { continue };
        let property = last.split('[').next().unwrap_or(last);
        // a password property of the site or of anything below it: the value is dropped here, and
        // only the script's own `<redacted>` (it has a value) counts, `<unset>` (it has none) and
        // anything else do not
        if property.to_ascii_lowercase().contains("pass") {
            site.has_password |= value.trim() == "<redacted>";
            continue;
        }
        let value = plain(value);
        let value = value.as_str();
        match (rest.as_slice(), property) {
            ([_], "ServerComment") => site.comment = value.trim().into(),
            ([_], "ServerBindings") => site.bindings.extend(parse_binding(value)),
            ([_], "ServerState") => site.state = state_name(value).into(),
            (["ROOT", _], "Path") => site.home = value.trim().into(),
            _ => {}
        }
    }
    sites
}

/// the facts of `machine` from both outputs.
pub fn facts(machine: &str, detected_at: u64, net_start: &str, dump: &str) -> HostingFacts {
    HostingFacts {
        machine: machine.into(),
        detected_at,
        services: parse_services(net_start),
        sites: parse_dump(dump),
        checks: Vec::new(),
        notes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "\
W3SVC/Class=IIsWebService
W3SVC/KeyType=IIsWebService
W3SVC/AnonymousUserName=IUSR_WEBHOST
W3SVC/AnonymousUserPass=<redacted>
W3SVC/1/Class=IIsWebServer
W3SVC/1/ServerComment=Intranet
W3SVC/1/ServerBindings[0]=:80:intranet.example.lan
W3SVC/1/ServerBindings[1]=192.0.2.20:8080:
W3SVC/1/ServerState=2
W3SVC/1/WAMUserPass=<redacted>
W3SVC/1/ROOT/Class=IIsWebVirtualDir
W3SVC/1/ROOT/Path=D:\\Sites\\intranet
W3SVC/1/ROOT/AccessFlags=513
W3SVC/1/ROOT/docs/Class=IIsWebVirtualDir
W3SVC/1/ROOT/docs/Path=D:\\Sites\\docs
W3SVC/1/Filters/Class=IIsFilters
W3SVC/2/Class=IIsWebServer
W3SVC/2/ServerComment=Test site
W3SVC/2/ServerBindings=:81:
W3SVC/2/ServerState=4
MSFTPSVC=absent
MSFTPSVC/Class=IIsFtpService
MSFTPSVC/1/Class=IIsFtpServer
MSFTPSVC/1/ServerComment=Files
MSFTPSVC/1/ServerBindings[0]=:21:
MSFTPSVC/1/ServerState=2
MSFTPSVC/1/ROOT/Path=D:\\Ftp\\root
MSFTPSVC/1/ROOT/UNCPassword=<redacted>
SMTPSVC=absent
";

    #[test]
    fn the_sites_come_out_of_the_dump() {
        let sites = parse_dump(DUMP);
        assert_eq!(sites.len(), 3);
        let web = &sites[0];
        assert_eq!((web.kind, web.id), (SiteKind::Web, 1));
        assert_eq!(web.comment, "Intranet");
        assert_eq!(web.state, "started");
        assert_eq!(web.home, "D:\\Sites\\intranet");
        assert_eq!(
            web.bindings,
            vec![
                Binding {
                    ip: "".into(),
                    port: 80,
                    host: "intranet.example.lan".into()
                },
                Binding {
                    ip: "192.0.2.20".into(),
                    port: 8080,
                    host: "".into()
                },
            ]
        );
        assert!(web.has_password);
        assert_eq!(sites[1].state, "stopped");
        assert_eq!(sites[1].bindings.len(), 1);
        assert!(!sites[1].has_password);
        let ftp = &sites[2];
        assert_eq!(
            (ftp.kind, ftp.id, ftp.home.as_str()),
            (SiteKind::Ftp, 1, "D:\\Ftp\\root")
        );
        assert!(ftp.has_password);
    }

    #[test]
    fn a_password_value_is_never_kept() {
        // the script redacts, but a line that is not redacted must not get through either
        let dump = "W3SVC/1/Class=IIsWebServer\n\
                    W3SVC/1/ServerComment=Intranet\n\
                    W3SVC/1/AnonymousUserPass=hunter2\n\
                    W3SVC/1/ROOT/WAMUserPass=s3cret-value\n";
        let sites = parse_dump(dump);
        // only the script's `<redacted>` says there is a password; a bare value is not trusted
        assert!(!sites[0].has_password);
        let json = serde_json::to_string(&facts("WEBHOST", 1, "", dump)).unwrap();
        assert!(
            !json.contains("hunter2") && !json.contains("s3cret-value"),
            "{json}"
        );
        assert!(!json.contains("redacted"));
        // `<unset>` says the property has no value
        let unset = parse_dump("W3SVC/1/WAMUserPass=<unset>\nW3SVC/1/AnonymousUserPass=<unset>\n");
        assert!(!unset[0].has_password);
        let set = parse_dump("W3SVC/1/WAMUserPass=<unset>\nW3SVC/1/AnonymousUserPass=<redacted>\n");
        assert!(set[0].has_password);
        // no password property at all
        let none = parse_dump("W3SVC/1/ServerComment=Intranet\n");
        assert!(!none[0].has_password);
    }

    #[test]
    fn control_characters_are_dropped_from_the_fields() {
        let dump = "W3SVC/1/ServerComment=In\u{1b}[31mtra\tnet\u{7}\n\
                    W3SVC/1/ServerBindings[0]=:80:a.exa\u{1b}mple.lan\n\
                    W3SVC/1/ROOT/Path=D:\\Si\u{8}tes\u{0}\n";
        let site = &parse_dump(dump)[0];
        assert_eq!(site.comment, "In[31mtranet");
        assert_eq!(site.bindings[0].host, "a.example.lan");
        assert_eq!(site.home, "D:\\Sites");
        assert!(!format!("{site:?}").contains("\\u{1b}"));
    }

    #[test]
    fn services_are_running_when_net_start_lists_them() {
        let out = "   IIS Admin Service\r\n   World Wide Web Publishing Service\r\n";
        let services = parse_services(out);
        let running = |name: &str| services.iter().find(|s| s.name == name).unwrap().running;
        assert!(running("IISADMIN") && running("W3SVC"));
        assert!(!running("MSFTPSVC") && !running("SMTPSVC"));
        assert!(parse_services("").iter().all(|s| !s.running));
    }

    #[test]
    fn odd_lines_are_ignored() {
        assert!(parse_dump("garbage\nW3SVC=absent\nW3SVC/x/ServerComment=a\n\n").is_empty());
        assert_eq!(parse_binding(":notaport:"), None);
    }

    #[test]
    fn a_check_reads_as_one_line_and_counts() {
        let check = |id: &str, status| CheckResult {
            id: id.into(),
            status,
            evidence: "seen".into(),
            fix_available: false,
        };
        assert_eq!(
            check("iis.samples", CheckStatus::Fail).line(),
            "FAIL iis.samples  seen"
        );
        assert_eq!(
            check("iis.printers", CheckStatus::NotApplicable).line(),
            "N/A  iis.printers  seen"
        );
        let long = CheckResult {
            evidence: "x".repeat(100),
            ..check("iis.samples", CheckStatus::Fail)
        };
        let cut = long.line_fit(76);
        assert_eq!(cut.chars().count(), 76);
        assert!(cut.starts_with("FAIL iis.samples  xxx") && cut.ends_with('\u{2026}'));
        assert_eq!(check("a", CheckStatus::Pass).line_fit(76), "PASS a  seen");
        let found = HostingFacts {
            checks: vec![
                check("a", CheckStatus::Fail),
                check("b", CheckStatus::Warn),
                check("c", CheckStatus::Fail),
                check("d", CheckStatus::Pass),
            ],
            ..HostingFacts::default()
        };
        assert_eq!(found.check_counts(), (2, 1));
        let json = serde_json::to_string(&check("x", CheckStatus::NotApplicable)).unwrap();
        assert!(json.contains("\"not_applicable\""), "{json}");
    }

    #[test]
    fn facts_survive_json() {
        let facts = facts("WEBHOST", 1_700_000_000, "   IIS Admin Service", DUMP);
        let json = serde_json::to_string(&facts).unwrap();
        assert_eq!(serde_json::from_str::<HostingFacts>(&json).unwrap(), facts);
        assert!(facts.iis_seen());
        assert!(!HostingFacts::default().iis_seen());
    }
}
