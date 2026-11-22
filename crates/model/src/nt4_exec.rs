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

/// one thing a front end can ask an iis 4 machine to do.
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
}

macro_rules! preset {
    ($id:expr, $label:expr, $template:expr, [$($arg:expr),*], $mutating:expr) => {
        Preset {
            id: $id,
            label: $label,
            template: $template,
            args: &[$($arg),*],
            mutating: $mutating,
        }
    };
}

/// every preset, read-only ones first.
pub const PRESETS: &[Preset] = &[
    preset!(
        "services-status",
        "IIS service status",
        r#"net start | findstr /i /c:"IIS Admin" /c:"World Wide Web" /c:"FTP Publishing""#,
        [],
        false
    ),
    preset!("net-start", "Running services", "net start", [], false),
    preset!(
        "adsutil-enum-w3svc",
        "Web service settings",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs ENUM W3SVC",
        [],
        false
    ),
    preset!(
        "adsutil-enum-site",
        "Web site settings",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs ENUM W3SVC/{site}",
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
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\adsutil.vbs ENUM MSFTPSVC/{site}",
        ["site"],
        false
    ),
    preset!("ver", "Windows version", "ver", [], false),
    preset!("ipconfig", "Network settings", "ipconfig /all", [], false),
    preset!(
        "w3svc-start",
        "Start the web service",
        "net start w3svc",
        [],
        true
    ),
    preset!(
        "w3svc-stop",
        "Stop the web service",
        "net stop w3svc",
        [],
        true
    ),
    preset!(
        "msftpsvc-start",
        "Start the FTP service",
        "net start msftpsvc",
        [],
        true
    ),
    preset!(
        "msftpsvc-stop",
        "Stop the FTP service",
        "net stop msftpsvc",
        [],
        true
    ),
    preset!(
        "iisadmin-start",
        "Start the IIS admin service",
        "net start iisadmin",
        [],
        true
    ),
    // iisadmin has dependents (web, ftp): /y stops them too
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
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\startweb.vbs -a {site}",
        ["site"],
        true
    ),
    preset!(
        "stopweb",
        "Stop a web site",
        r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples\stopweb.vbs -a {site}",
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

    const ADMIN: &str = r"cscript //nologo %SystemRoot%\system32\inetsrv\adminsamples";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
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
            let placeholders = preset.template.matches('{').count();
            assert_eq!(placeholders, preset.args.len(), "{}", preset.id);
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
        for preset in PRESETS.iter().filter(|p| p.template.contains("cscript")) {
            assert!(preset.template.starts_with(ADMIN), "{}", preset.id);
        }
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
