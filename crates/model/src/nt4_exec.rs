//! running commands on windows nt 4 machines: the checks on a command, the iis 4 presets and the
//! decoder for what the machine prints.
//!
//! a preset's template is the text the machine's `cmd` runs. it names its own `%SystemRoot%`, so a
//! runner must not double the percent signs of a rendered preset the way it does for a free-form
//! command.

/// the account the commands run as; apply creates it as a domain admin.
pub const USER: &str = "lanlord-exec";

/// the characters of its generated password.
pub const PASSWORD_LEN: usize = 24;

/// whether the settings have a domain controller, so apply makes the exec account. a workgroup
/// or plain file server has none. only the list `nt4-servers` counts: the module does nothing
/// without it, and an empty or null list has no controller.
pub fn wants_account(settings: &serde_yaml_ng::Value) -> bool {
    use serde_yaml_ng::Value;
    match settings.get("nt4-servers") {
        Some(Value::Sequence(items)) => items
            .iter()
            .any(|item| item.get("role").and_then(Value::as_str) == Some("pdc")),
        _ => false,
    }
}

/// the most bytes a command may have.
pub const MAX_COMMAND_BYTES: usize = 2000;

/// the most bytes one preset argument may have.
const MAX_ARG_BYTES: usize = 64;

/// a free-form command: not empty, at most [`MAX_COMMAND_BYTES`] and without CR, LF or NUL.
pub fn check_command(command: &str) -> Result<(), &'static str> {
    if command.trim().is_empty() {
        return Err("must not be empty");
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err("is longer than 2000 bytes");
    }
    if command.contains(['\r', '\n', '\0']) {
        return Err("must be one line without NUL");
    }
    Ok(())
}

/// one thing a front end can ask an iis 4 machine to do. a preset that filters the metabase
/// listing (`| findstr ...`) exits with the code of `findstr` (0 when lines are left, 1 when
/// none are), not of `cscript`: the pipe is the last command of the batch file, and cmd on nt 4
/// has no way to keep the first command's code without writing the unfiltered listing to disk.
/// likewise `iisadmin-start` exits with the code of its closing `findstr` (a failed `net start`
/// is masked), and `startweb` and `stopweb` with the code of the `adsutil GET` that reads the
/// site's state back, not of the script that started or stopped it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    /// the command, with `{name}` where an argument goes.
    pub template: &'static str,
    /// the names of the arguments, in the order a caller gives them.
    pub args: &'static [&'static str],
    /// does it change the machine? a front end asks before running one that does.
    pub mutating: bool,
    /// the name of a whitelisted script (see [`SCRIPTS`]) that is put on the machine for the run;
    /// the template then names it as `%LL_SCRIPT%`.
    pub script: Option<&'static str>,
}

macro_rules! preset {
    ($id:expr, $label:expr, $template:expr, [$($arg:expr),*], $mutating:expr) => {
        Preset {
            id: $id,
            label: $label,
            template: $template,
            args: &[$($arg),*],
            mutating: $mutating,
            script: None,
        }
    };
    ($id:expr, $label:expr, $template:expr, $script:expr) => {
        Preset {
            id: $id,
            label: $label,
            template: $template,
            args: &[],
            mutating: false,
            script: Some($script),
        }
    };
}

/// the start or stop preset of a service: `service_preset!(start, "w3svc", "web")`.
macro_rules! service_preset {
    (start, $service:expr, $what:expr) => {
        preset!(
            concat!($service, "-start"),
            concat!("Start the ", $what, " service"),
            concat!("net start ", $service),
            [],
            true
        )
    };
    (stop, $service:expr, $what:expr) => {
        preset!(
            concat!($service, "-stop"),
            concat!("Stop the ", $what, " service"),
            concat!("net stop ", $service),
            [],
            true
        )
    };
}

/// every preset, read-only ones first.
pub const PRESETS: &[Preset] = &[
    preset!(
        "services-status",
        "IIS service status",
        r#"net start | findstr /i /c:"IIS Admin" /c:"World Wide Web" /c:"FTP Publishing" /c:"SMTP Service""#,
        [],
        false
    ),
    preset!("net-start", "Running services", "net start", [], false),
    preset!(
        "iis-dump",
        "Dump the IIS metabase",
        r#"cscript //nologo "%LL_SCRIPT%""#,
        "lldump.vbs"
    ),
    preset!(
        "adsutil-enum-w3svc",
        "Web service settings",
        r#"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs ENUM W3SVC | findstr /v /r /i /c:"^ *[A-Za-z]*Pass[A-Za-z]* *:""#,
        [],
        false
    ),
    preset!(
        "adsutil-enum-site",
        "Web site settings",
        r#"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs ENUM W3SVC/{site} | findstr /v /r /i /c:"^ *[A-Za-z]*Pass[A-Za-z]* *:""#,
        ["site"],
        false
    ),
    preset!(
        "adsutil-site-bindings",
        "Web site bindings",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs GET W3SVC/{site}/ServerBindings",
        ["site"],
        false
    ),
    preset!(
        "adsutil-site-state",
        "Web site state",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs GET W3SVC/{site}/ServerState",
        ["site"],
        false
    ),
    preset!(
        "adsutil-enum-ftp-site",
        "FTP site settings",
        r#"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs ENUM MSFTPSVC/{site} | findstr /v /r /i /c:"^ *[A-Za-z]*Pass[A-Za-z]* *:""#,
        ["site"],
        false
    ),
    preset!("ver", "Windows version", "ver", [], false),
    preset!("ipconfig", "Network settings", "ipconfig /all", [], false),
    service_preset!(start, "w3svc", "web"),
    service_preset!(stop, "w3svc", "web"),
    service_preset!(start, "msftpsvc", "FTP"),
    service_preset!(stop, "msftpsvc", "FTP"),
    service_preset!(start, "smtpsvc", "SMTP"),
    service_preset!(stop, "smtpsvc", "SMTP"),
    preset!(
        "smtpsvc-restart",
        "Restart the SMTP service",
        "net stop smtpsvc & net start smtpsvc",
        [],
        true
    ),
    // `&` runs the next command whatever the last one returned, so a service that is not
    // installed ("service name is invalid") does not stop the rest; the listing ends it
    preset!(
        "iisadmin-start",
        "Start the IIS admin service and its dependents",
        r#"net start iisadmin & net start w3svc & net start msftpsvc & net start smtpsvc & net start | findstr /i /c:"IIS Admin" /c:"World Wide Web" /c:"FTP Publishing" /c:"SMTP Service""#,
        [],
        true
    ),
    // iisadmin has dependents (web, ftp, smtp): /y stops them too
    preset!(
        "iisadmin-stop",
        "Stop the IIS admin service and its dependents",
        "net stop iisadmin /y",
        [],
        true
    ),
    preset!(
        "startweb",
        "Start a web site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\startweb.vbs -a {site} & cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs GET W3SVC/{site}/ServerState",
        ["site"],
        true
    ),
    preset!(
        "stopweb",
        "Stop a web site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\stopweb.vbs -a {site} & cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs GET W3SVC/{site}/ServerState",
        ["site"],
        true
    ),
    preset!(
        "pauseweb",
        "Pause a web site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\pauseweb.vbs -a {site}",
        ["site"],
        true
    ),
    preset!(
        "contweb",
        "Continue a paused web site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\contweb.vbs -a {site}",
        ["site"],
        true
    ),
    preset!(
        "startftp",
        "Start an FTP site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\startftp.vbs -a {site}",
        ["site"],
        true
    ),
    preset!(
        "stopftp",
        "Stop an FTP site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\stopftp.vbs -a {site}",
        ["site"],
        true
    ),
    preset!(
        "mkwebdir",
        "Create a virtual directory",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\mkwebdir.vbs -c localhost -w {site} -v {name},{path}",
        ["site", "name", "path"],
        true
    ),
];

/// the presets that are not about hosting; they stay under the nt4 domain and its remote
/// commands. every other preset (the iis ones, and any added later) needs the hosting component.
const GENERAL: &[&str] = &["net-start", "ver", "ipconfig"];

/// is this preset one of the hosting presets, that needs the hosting component switched on and
/// manage rights on hosting? an unknown id is not: it is refused as unknown before it matters.
pub fn is_hosting(id: &str) -> bool {
    find(id).is_some() && !GENERAL.contains(&id)
}

/// a script the repo ships that may be put on a machine; nothing else is ever uploaded as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Script {
    pub name: &'static str,
    pub label: &'static str,
    pub body: &'static str,
}

/// the whitelist of scripts a run may put on a machine.
pub const SCRIPTS: &[Script] = &[Script {
    name: "lldump.vbs",
    label: "Dump the IIS metabase as key=value lines",
    body: include_str!("../nt4-scripts/lldump.vbs"),
}];

/// the most data files one run may put on a machine.
pub const MAX_PUT_FILES: usize = 4;

/// the most bytes one data file may have.
pub const MAX_PUT_FILE_BYTES: usize = 1 << 20;

/// the script with this exact name from the whitelist. a name with a path in it (`/`, `\\`, `..`
/// or a drive letter) is refused as such, so a traversal never gets as far as a lookup.
pub fn find_script(name: &str) -> Result<&'static Script, &'static str> {
    if name.is_empty() || name.len() > MAX_ARG_BYTES {
        return Err("must be 1 to 64 characters");
    }
    if name.contains(['/', '\\', ':']) || name.contains("..") || name.starts_with('.') {
        return Err("must be a plain script name, not a path");
    }
    SCRIPTS
        .iter()
        .find(|script| script.name == name)
        .ok_or("is not a script that may be put on a machine")
}

/// whether `text` is base64 (standard alphabet, padded) of at most [`MAX_PUT_FILE_BYTES`] bytes.
pub fn check_file(text: &str) -> Result<(), &'static str> {
    if text.len() > MAX_PUT_FILE_BYTES.div_ceil(3) * 4 {
        return Err("is longer than 1 MiB");
    }
    // the padding is part of the length, so the decoded size is exact
    let padding = text.bytes().rev().take_while(|&b| b == b'=').count().min(2);
    if (text.len() / 4 * 3).saturating_sub(padding) > MAX_PUT_FILE_BYTES {
        return Err("is longer than 1 MiB");
    }
    if !text.len().is_multiple_of(4)
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    {
        return Err("must be base64");
    }
    Ok(())
}

/// the command that runs a put script with these arguments. the machine's `%LL_SCRIPT%` is the
/// staged copy; `%LL_DATA1%` and so on are the data files.
pub fn render_put(script: &str, args: &[String]) -> Result<String, &'static str> {
    find_script(script)?;
    let mut command = String::from(r#"cscript //nologo "%LL_SCRIPT%""#);
    for arg in args {
        check_arg(arg)?;
        command.push(' ');
        command.push_str(arg);
    }
    Ok(command)
}

/// the preset with this id.
pub fn find(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// an argument: 1 to 64 characters of `[A-Za-z0-9_./:\-]`, not starting with `-` or `/` (that
/// would be an option, as `//S` is to cscript) and without `,` (that would add a name,path pair).
pub fn check_arg(arg: &str) -> Result<(), &'static str> {
    if arg.is_empty() || arg.len() > MAX_ARG_BYTES {
        return Err("must be 1 to 64 characters");
    }
    if arg.starts_with(['-', '/']) {
        return Err("must not start with - or /");
    }
    if !arg
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_./:\\-".contains(&b))
    {
        return Err("may only hold letters, digits and _ . / : \\ -");
    }
    Ok(())
}

/// the command a preset runs with these arguments, one per name in the preset's `args`.
pub fn render(id: &str, args: &[String]) -> Result<String, &'static str> {
    let preset = find(id).ok_or("is not a known preset")?;
    if args.len() != preset.args.len() {
        return Err("needs a different number of arguments");
    }
    let mut command = preset.template.to_string();
    for (name, arg) in preset.args.iter().zip(args) {
        check_arg(arg)?;
        command = command.replace(&format!("{{{name}}}"), arg);
    }
    Ok(command)
}

/// must a front end ask before running this? `preset` is the id of a preset, or `None` for a
/// free-form command. a free-form command always asks, and so does an unknown preset.
pub fn needs_confirmation(preset: Option<&str>) -> bool {
    preset.is_none_or(|id| find(id).is_none_or(|preset| preset.mutating))
}

/// the characters of code page 437 from 0x80 up.
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// what replaces the value of a secret property in output.
const HIDDEN: &[u8] = b"(hidden)";

/// a metabase property line, such as `AnonymousUserPass : (STRING) "..."`: returns the
/// property name, the type and where the value starts.
fn property(line: &[u8]) -> Option<(&[u8], &[u8], usize)> {
    let skip = |at: usize| {
        at + line[at..]
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count()
    };
    let name = skip(0);
    let word = line[name..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
        .count();
    let colon = skip(name + word);
    if word == 0 || line.get(colon) != Some(&b':') {
        return None;
    }
    let tag = skip(colon + 1);
    if line.get(tag) != Some(&b'(') {
        return None;
    }
    let close = tag + line[tag..].iter().position(|&b| b == b')')?;
    Some((&line[name..name + word], &line[tag + 1..close], close + 1))
}

/// does a property hold a secret? a name with `pass` in any case, unless the value is a flag or
/// a number, which cannot hold one.
fn is_secret(name: &[u8], kind: &[u8]) -> bool {
    name.to_ascii_lowercase().windows(4).any(|w| w == b"pass")
        && !kind.eq_ignore_ascii_case(b"BOOLEAN")
        && !kind.eq_ignore_ascii_case(b"INTEGER")
}

/// the output with the value of every secret property (a name holding `Pass`, as
/// `AnonymousUserPass` or `WAMUserPass`) replaced by `(hidden)`, line by line. the indented
/// lines that continue a list value are hidden too, up to the next property line.
pub fn redact_secrets(output: &[u8]) -> Vec<u8> {
    redact_lines(output, &mut false)
}

/// `redact_secrets` for output that comes in parts: `continued` says the last line was a secret
/// whose value goes on in the lines after it.
fn redact_lines(output: &[u8], continued: &mut bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(output.len());
    for raw in output.split_inclusive(|&b| b == b'\n') {
        let body = raw.strip_suffix(b"\n").unwrap_or(raw);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        let prop = property(body);
        if let Some((_, _, start)) = prop.filter(|(name, kind, _)| is_secret(name, kind)) {
            out.extend_from_slice(&body[..start]);
            out.push(b' ');
            out.extend_from_slice(HIDDEN);
            out.extend_from_slice(&raw[body.len()..]);
            *continued = true;
        } else if *continued
            && prop.is_none()
            && body.first().is_some_and(u8::is_ascii_whitespace)
            && !body.trim_ascii().is_empty()
        {
            out.extend_from_slice(HIDDEN);
            out.extend_from_slice(&raw[body.len()..]);
        } else {
            *continued = false;
            out.extend_from_slice(raw);
        }
    }
    out
}

/// redacts a stream that arrives in chunks: a line is held until it is complete.
#[derive(Debug, Default)]
pub struct LiveRedactor {
    pending: Vec<u8>,
    continued: bool,
}

impl LiveRedactor {
    /// take a chunk; returns the complete lines it finished, redacted.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        match self.pending.iter().rposition(|&b| b == b'\n') {
            Some(end) => {
                let rest = self.pending.split_off(end + 1);
                let done = std::mem::replace(&mut self.pending, rest);
                redact_lines(&done, &mut self.continued)
            }
            None => Vec::new(),
        }
    }

    /// the unfinished last line, redacted.
    pub fn finish(&mut self) -> Vec<u8> {
        redact_lines(&std::mem::take(&mut self.pending), &mut self.continued)
    }
}

/// text from a machine's oem code page 437 output. nt 4 prints in it by default.
pub fn decode_cp437(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                CP437_HIGH[usize::from(b - 0x80)]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_domain_controller_wants_the_account() {
        let want = |text: &str| super::wants_account(&serde_yaml_ng::from_str(text).unwrap());
        assert!(want("nt4-servers:\n  - {name: a, role: pdc}\n"));
        assert!(want(
            "nt4-servers:\n  - {name: f, role: workgroup}\n  - {name: a, role: pdc}\n"
        ));
        assert!(!want("nt4-servers:\n  - {name: f, role: workgroup}\n"));
        assert!(!want("nt4-servers: []\n"));
        // the module does nothing without the list, so none, null and empty want no account
        assert!(!want("nt4-domain:\n  admin_password: x\n"));
        assert!(!want("nt4-domain:\n  mode: domain\n"));
        assert!(!want("nt4-servers:\n"));
        assert!(!want("nt4-servers: null\n"));
        assert!(!want("nt4-servers: ~\n"));
        assert!(!want("dns: {}\n"));
        assert!(!want("{}\n"));
    }

    use super::*;

    #[test]
    fn the_iis_presets_are_hosting_and_the_general_ones_are_not() {
        for preset in PRESETS {
            assert_eq!(
                is_hosting(preset.id),
                !GENERAL.contains(&preset.id),
                "{}",
                preset.id
            );
        }
        for general in GENERAL {
            assert!(find(general).is_some(), "{general} is not a preset");
        }
        assert!(is_hosting("startweb") && is_hosting("services-status") && is_hosting("iis-dump"));
        assert!(!is_hosting("ver") && !is_hosting("nope"));
    }

    const FILTER: &str = r#" | findstr /v /r /i /c:"^ *[A-Za-z]*Pass[A-Za-z]* *:""#;

    const ADMIN: &str = r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples";

    #[test]
    fn only_a_shipped_script_name_is_found() {
        assert_eq!(find_script("lldump.vbs").unwrap().name, "lldump.vbs");
        for bad in [
            "",
            "other.vbs",
            "../lldump.vbs",
            "..\\lldump.vbs",
            "/lldump.vbs",
            "\\lldump.vbs",
            "C:lldump.vbs",
            "C:\\lldump.vbs",
            "nt4-scripts/lldump.vbs",
            ".lldump.vbs",
            "lldump.vbs\0",
            "LLDUMP.VBS",
        ] {
            assert!(find_script(bad).is_err(), "{bad:?}");
        }
        for script in SCRIPTS {
            assert!(find_script(script.name).is_ok());
            assert!(!script.body.is_empty());
        }
    }

    #[test]
    fn a_script_preset_names_a_shipped_script() {
        for preset in PRESETS {
            if let Some(name) = preset.script {
                assert!(find_script(name).is_ok(), "{}", preset.id);
                assert!(preset.template.contains("%LL_SCRIPT%"), "{}", preset.id);
                assert!(!preset.mutating && preset.args.is_empty(), "{}", preset.id);
            }
        }
        assert_eq!(find("iis-dump").unwrap().script, Some("lldump.vbs"));
    }

    #[test]
    fn a_put_command_runs_the_staged_script_with_plain_arguments() {
        assert_eq!(
            render_put("lldump.vbs", &[]).unwrap(),
            r#"cscript //nologo "%LL_SCRIPT%""#
        );
        assert!(render_put("lldump.vbs", &["a".into(), "%LL_DATA1%".into()]).is_err());
        assert!(render_put("lldump.vbs", &["-x".into()]).is_err());
        assert!(render_put("../lldump.vbs", &[]).is_err());
    }

    #[test]
    fn a_data_file_is_base64_within_the_limit() {
        assert!(check_file("").is_ok());
        assert!(check_file("aGk=").is_ok());
        assert!(check_file("aGk").is_err());
        assert!(check_file("a!k=").is_err());
        assert!(check_file(&"A".repeat(MAX_PUT_FILE_BYTES.div_ceil(3) * 4 + 4)).is_err());
        // exactly 1 MiB decoded passes, one byte more does not
        let limit = MAX_PUT_FILE_BYTES.div_ceil(3) * 4;
        assert!(check_file(&format!("{}==", "A".repeat(limit - 2))).is_ok());
        assert!(check_file(&format!("{}=", "A".repeat(limit - 1))).is_err());
        assert!(check_file(&"A".repeat(limit)).is_err());
    }

    #[test]
    fn the_dump_script_never_reads_a_password_property() {
        let body = SCRIPTS[0].body;
        // drop comments: text after a quote that is outside a string literal
        let strip = |line: &str| -> String {
            let mut quoted = false;
            for (i, c) in line.char_indices() {
                match c {
                    '"' => quoted = !quoted,
                    '\'' if !quoted => return line[..i].to_string(),
                    _ => {}
                }
            }
            line.to_string()
        };
        let stripped: Vec<String> = body.lines().map(strip).collect();
        let code: Vec<&str> = stripped.iter().map(String::as_str).collect();
        let is_get = |l: &str| {
            let lower = l.to_ascii_lowercase();
            lower.match_indices(".get").any(|(i, _)| {
                let rest = &lower[i + 4..];
                rest.trim_start().starts_with('(')
            })
        };
        let gets: Vec<&&str> = code.iter().filter(|l| is_get(l)).collect();
        assert_eq!(gets.len(), 2, "two Gets: SafeGet and HasSecret");
        let text = code.join("\n");
        // SafeGet looks at the name before it reads
        let safe = &text[text.find("Function SafeGet").unwrap()..];
        let guard = safe.find("If IsSecret(sName) Then Exit Function").unwrap();
        assert!(guard < safe.find(".Get(").unwrap());
        // HasSecret answers a Boolean: it prints nothing and returns nothing but its tests
        let has = &text[text.find("Function HasSecret").unwrap()..];
        let has = &has[..has.find("End Function").unwrap()];
        assert!(!has.contains("Echo"));
        for line in has.lines().filter(|l| l.contains("HasSecret =")) {
            let value = line.split('=').nth(1).unwrap().trim();
            assert!(
                ["True", "False"].contains(&value) || value.starts_with('('),
                "{line}"
            );
        }
        assert!(text.contains(r#"InStr(1, sName, "pass", 1)"#));
        assert!(!text.contains(".GetEx") && !text.contains(".GetInfo"));
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn enum_presets_drop_password_lines_on_the_machine() {
        for id in [
            "adsutil-enum-w3svc",
            "adsutil-enum-site",
            "adsutil-enum-ftp-site",
        ] {
            let preset = find(id).unwrap();
            assert!(!preset.mutating);
            assert!(preset.template.ends_with(FILTER), "{id}");
        }
    }

    #[test]
    fn secret_values_are_hidden() {
        let text = b"KeyType : (STRING) \"IIsWebServer\"\r\n  AnonymousUserPass   : (STRING) \"s3cret\"\r\nWAMUserPass: (STRING) \"x\"\nUNCPassword : (STRING) \"y\"\nAnonymousPasswordSync : (BOOLEAN) True\nServerComment : (STRING) \"a Pass b\"\nlast Pass : (STRING) \"z\"";
        let out = String::from_utf8(redact_secrets(text)).unwrap();
        assert_eq!(
            out,
            "KeyType : (STRING) \"IIsWebServer\"\r\n  AnonymousUserPass   : (STRING) (hidden)\r\nWAMUserPass: (STRING) (hidden)\nUNCPassword : (STRING) (hidden)\nAnonymousPasswordSync : (BOOLEAN) True\nServerComment : (STRING) \"a Pass b\"\nlast Pass : (STRING) \"z\""
        );
        assert!(!out.contains("s3cret"));
    }

    #[test]
    fn every_secret_type_is_hidden_in_any_case() {
        let text = b"password : (EXPANDSZ) \"s3cret\"\nUserPassList : (LIST) (2 Items)\n  \"s3cret1\"\n\t\"s3cret2\"\nServerComment : (STRING) \"x\"\n  \"kept\"\n[/W3SVC/1/ROOT/Passport]\nKeyType : (STRING) \"IIsWebVirtualDir\"\n";
        let out = String::from_utf8(redact_secrets(text)).unwrap();
        assert_eq!(
            out,
            "password : (EXPANDSZ) (hidden)\nUserPassList : (LIST) (hidden)\n(hidden)\n(hidden)\nServerComment : (STRING) \"x\"\n  \"kept\"\n[/W3SVC/1/ROOT/Passport]\nKeyType : (STRING) \"IIsWebVirtualDir\"\n"
        );
        assert!(!out.contains("s3cret"));
    }

    #[test]
    fn a_list_secret_split_across_chunks_is_hidden() {
        let mut live = LiveRedactor::default();
        let mut out = live.push(b"UserPass : (LIST) (1 Items)\n  \"s3");
        out.extend(live.push(b"cret\"\nNext : (STRING) \"a\"\n"));
        out.extend(live.finish());
        assert_eq!(
            out,
            b"UserPass : (LIST) (hidden)\n(hidden)\nNext : (STRING) \"a\"\n"
        );
    }

    #[test]
    fn a_secret_split_across_chunks_is_hidden() {
        let mut live = LiveRedactor::default();
        let mut out = live.push(b"Anonymous");
        out.extend(live.push(b"UserPass : (STRING) \"s3c"));
        out.extend(live.push(b"ret\"\nnext"));
        out.extend(live.finish());
        assert_eq!(out, b"AnonymousUserPass : (STRING) (hidden)\nnext");
    }

    #[test]
    fn commands_are_one_short_line() {
        assert_eq!(check_command("ver"), Ok(()));
        assert!(check_command("").is_err());
        assert!(check_command("   ").is_err());
        assert!(check_command("a\nb").is_err());
        assert!(check_command("a\rb").is_err());
        assert!(check_command("a\0b").is_err());
        assert_eq!(check_command(&"x".repeat(2000)), Ok(()));
        assert!(check_command(&"x".repeat(2001)).is_err());
    }

    #[test]
    fn preset_ids_are_unique_and_templates_match_their_arguments() {
        for (i, preset) in PRESETS.iter().enumerate() {
            assert!(PRESETS[i + 1..].iter().all(|p| p.id != preset.id));
            // a name may appear more than once (the site of startweb is also read back)
            let named: usize = preset
                .args
                .iter()
                .map(|name| preset.template.matches(&format!("{{{name}}}")).count())
                .sum();
            assert_eq!(named, preset.template.matches('{').count(), "{}", preset.id);
            for name in preset.args {
                assert!(preset.template.contains(&format!("{{{name}}}")));
            }
        }
    }

    #[test]
    fn presets_render() {
        assert_eq!(render("ver", &[]).unwrap(), "ver");
        assert_eq!(
            render("adsutil-site-state", &args(&["1"])).unwrap(),
            r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs GET W3SVC/1/ServerState"
        );
        assert_eq!(
            render("mkwebdir", &args(&["1", "docs", "C:/inetpub/docs"])).unwrap(),
            r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\mkwebdir.vbs -c localhost -w 1 -v docs,C:/inetpub/docs"
        );
        assert!(render("mkwebdir", &args(&["1", "docs", r"C:\inetpub\docs"])).is_ok());
        assert_eq!(
            render("iisadmin-stop", &[]).unwrap(),
            "net stop iisadmin /y"
        );
        assert_eq!(
            render("smtpsvc-restart", &[]).unwrap(),
            "net stop smtpsvc & net start smtpsvc"
        );
        let up = render("iisadmin-start", &[]).unwrap();
        for service in ["iisadmin", "w3svc", "msftpsvc", "smtpsvc"] {
            assert!(up.contains(&format!("net start {service}")), "{service}");
        }
        assert!(!up.contains("&&"));
        assert!(render("services-status", &[])
            .unwrap()
            .contains("SMTP Service"));
        let web = render("startweb", &args(&["1"])).unwrap();
        assert!(web.ends_with("adsutil.vbs GET W3SVC/1/ServerState"));
        assert!(render("stopweb", &args(&["2"]))
            .unwrap()
            .ends_with("W3SVC/2/ServerState"));
    }

    #[test]
    fn presets_refuse_bad_input() {
        assert!(render("nope", &[]).is_err());
        assert!(render("startweb", &[]).is_err());
        assert!(render("ver", &args(&["1"])).is_err());
        for bad in [
            "",
            "1 & del *",
            "1;x",
            "a,b",
            "-c",
            "--computer",
            "//S",
            "/x",
            "a\"b",
            "%PATH%",
            "1\n2",
            "a|b",
            &"9".repeat(65),
        ] {
            assert!(render("startweb", &args(&[bad])).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_scripts_live_in_adminsamples() {
        assert!(ADMIN.ends_with(r"inetsrv\adminsamples"));
        for preset in PRESETS
            .iter()
            .filter(|p| p.template.contains("cscript") && p.script.is_none())
        {
            assert!(preset.template.starts_with(ADMIN), "{}", preset.id);
            // every script named is one in adminsamples
            let scripts = preset.template.matches("cscript").count();
            assert_eq!(
                scripts,
                preset.template.matches(ADMIN).count(),
                "{}",
                preset.id
            );
        }
    }

    /// the metabase properties a preset may GET, by the last segment of the path.
    const GET_ALLOWED: &[&str] = &["ServerState", "ServerBindings"];

    /// why a template could print a password property, or `Ok`. every command of the template
    /// (they are joined with `&`) is looked at: a GET must name an allowed property, an ENUM
    /// must end in [`FILTER`], and `Pass` may only appear inside [`FILTER`].
    fn password_problem(template: &str) -> Result<(), String> {
        if template.replace(FILTER, "").to_lowercase().contains("pass") {
            return Err("names a password outside the filter".into());
        }
        for command in template.split('&') {
            let words: Vec<&str> = command.split_whitespace().collect();
            for (i, word) in words.iter().enumerate() {
                match *word {
                    verb if verb.eq_ignore_ascii_case("GET") => {
                        let path = words.get(i + 1).ok_or("GET without a path")?;
                        let last = path.rsplit('/').next().unwrap_or_default();
                        if !GET_ALLOWED.contains(&last) {
                            return Err(format!("GET of {last}"));
                        }
                    }
                    verb if verb.eq_ignore_ascii_case("ENUM")
                        && !command.trim_end().ends_with(FILTER) =>
                    {
                        return Err("ENUM without the filter".into());
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    #[test]
    fn no_preset_reads_a_password() {
        for preset in PRESETS {
            assert_eq!(password_problem(preset.template), Ok(()), "{}", preset.id);
        }
    }

    #[test]
    fn the_password_check_rejects_bad_templates() {
        let tool = r"cscript //nologo x\adsutil.vbs";
        let get = format!("{tool} GET W3SVC/1/");
        assert!(password_problem(&format!("{get}ServerState")).is_ok());
        for bad in [
            format!("{get}ServerState & {get}AnonymousUserPass"),
            format!("{get}AnonymousUserPass"),
            format!("{tool} get W3SVC/1/KeyType"),
            format!("{tool} enum W3SVC"),
            format!("{get}ServerState/../x"),
            format!("{get}ServerState & echo PASSWORD"),
            format!("{tool} ENUM W3SVC"),
            format!("{tool} ENUM W3SVC & echo x{FILTER}"),
            format!("{tool} ENUM W3SVC/1 & {get}ServerState{FILTER}"),
        ] {
            assert!(password_problem(&bad).is_err(), "{bad}");
        }
        // a lowercase get of a property that holds no password is refused for the GET rule
        // itself, not by the "Pass" check, so it exercises the case-insensitive verb
        assert_eq!(
            password_problem(&format!("{tool} get W3SVC/1/KeyType")),
            Err("GET of KeyType".to_string())
        );
        let ok = format!("{tool} ENUM W3SVC{FILTER}");
        assert!(password_problem(&ok).is_ok());
    }

    #[test]
    fn confirmation_follows_what_changes() {
        assert!(needs_confirmation(None));
        assert!(needs_confirmation(Some("unknown")));
        assert!(needs_confirmation(Some("w3svc-stop")));
        assert!(!needs_confirmation(Some("ver")));
        for preset in PRESETS {
            assert_eq!(needs_confirmation(Some(preset.id)), preset.mutating);
        }
        assert!(PRESETS.iter().any(|p| !p.mutating));
    }

    #[test]
    fn cp437_decodes() {
        assert_eq!(decode_cp437(b"Hello\r\n"), "Hello\r\n");
        assert_eq!(decode_cp437(&[0x80, 0x81, 0x82]), "Çüé");
        assert_eq!(decode_cp437(&[0xC9, 0xCD, 0xBB]), "╔═╗");
        assert_eq!(decode_cp437(&[0xE1, 0xF8, 0xFB, 0xFE]), "ß°√■");
        assert_eq!(decode_cp437(&[0xFF]), "\u{a0}");
        assert_eq!(decode_cp437(&[]), "");
    }

    #[test]
    fn cp437_table_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for c in CP437_HIGH {
            assert!(seen.insert(c), "{c}");
        }
    }
}
