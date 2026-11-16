//! `install-launcher` and `uninstall-launcher`: put LANlord in the app menu (linux), in
//! ~/Applications (macos) or in the start menu (windows), pointing at the running binary.
//!
//! every file written is listed in a manifest in the lanlord config dir, and uninstall removes
//! exactly those files, so a launcher made by hand is never touched. install refuses to replace
//! a file it did not write unless `--force` is given.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

/// the platform a plan is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// the environment a plan depends on, read once so tests can make their own.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    /// the lanlord config dir, which holds the manifest (and the windows icon).
    pub config_dir: Option<PathBuf>,
}

impl Env {
    fn current() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: var("HOME").or_else(|| var("USERPROFILE")),
            xdg_data_home: var("XDG_DATA_HOME"),
            appdata: var("APPDATA"),
            config_dir: lanlord_core::paths::config_dir(),
        }
    }
}

/// what one planned file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Bytes(Vec<u8>),
    /// a script that must be executable.
    Script(String),
    /// a windows shortcut to `target` with `args`, showing `icon`.
    Shortcut {
        target: PathBuf,
        args: String,
        icon: PathBuf,
    },
}

/// a file install-launcher writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub path: PathBuf,
    pub content: Content,
}

/// the full-bleed square icon the app draws for itself; linux taskbars show it edge to edge.
const ICON_PNG: &[u8] = include_bytes!("../../ui/assets/icon.png");
const ICON_32: &[u8] = include_bytes!("../../../assets/icon-32.png");
#[cfg(target_os = "macos")]
const ICON_ICNS: &[u8] = include_bytes!("../../../assets/icon.icns");
#[cfg(not(target_os = "macos"))]
const ICON_ICNS: &[u8] = &[];
#[cfg(windows)]
const ICON_ICO: &[u8] = include_bytes!("../../../assets/icon.ico");
#[cfg(not(windows))]
const ICON_ICO: &[u8] = &[];

/// the manifest of files install-launcher wrote.
fn manifest_path(env: &Env) -> Result<PathBuf> {
    env.config_dir
        .as_ref()
        .map(|d| d.join("launcher-files"))
        .ok_or_else(|| anyhow!("can't find the lanlord config dir"))
}

/// the files that make up the launcher on `os`, for the binary at `exe`.
pub fn plan(os: Os, env: &Env, exe: &Path) -> Result<Vec<Planned>> {
    let home = || {
        env.home
            .clone()
            .ok_or_else(|| anyhow!("can't find your home folder"))
    };
    let file = |path: PathBuf, content: Content| Planned { path, content };
    Ok(match os {
        Os::Linux => {
            let data = match &env.xdg_data_home {
                Some(d) => d.clone(),
                None => home()?.join(".local").join("share"),
            };
            let apps = data.join("applications");
            let icons = data.join("icons").join("hicolor");
            let icon = icons.join("512x512").join("apps").join("lanlord.png");
            vec![
                file(
                    apps.join("lanlord.desktop"),
                    Content::Bytes(desktop_entry(exe, &icon, false).into_bytes()),
                ),
                file(
                    apps.join("lanlord-tui.desktop"),
                    Content::Bytes(desktop_entry(exe, &icon, true).into_bytes()),
                ),
                file(icon.clone(), Content::Bytes(ICON_PNG.to_vec())),
                file(
                    icons.join("32x32").join("apps").join("lanlord.png"),
                    Content::Bytes(ICON_32.to_vec()),
                ),
            ]
        }
        Os::Mac => {
            let contents = home()?
                .join("Applications")
                .join("LANlord.app")
                .join("Contents");
            vec![
                file(
                    contents.join("Info.plist"),
                    Content::Bytes(info_plist().into_bytes()),
                ),
                file(
                    contents.join("MacOS").join("LANlord"),
                    Content::Script(mac_script(exe)),
                ),
                file(
                    contents.join("Resources").join("icon.icns"),
                    Content::Bytes(ICON_ICNS.to_vec()),
                ),
            ]
        }
        Os::Windows => {
            let appdata = env
                .appdata
                .clone()
                .ok_or_else(|| anyhow!("APPDATA is not set"))?;
            let config = env
                .config_dir
                .clone()
                .ok_or_else(|| anyhow!("can't find the lanlord config dir"))?;
            let programs = appdata
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("LANlord");
            let icon = config.join("lanlord.ico");
            let shortcut = |args: &str| Content::Shortcut {
                target: exe.to_path_buf(),
                args: args.into(),
                icon: icon.clone(),
            };
            vec![
                file(icon.clone(), Content::Bytes(ICON_ICO.to_vec())),
                file(programs.join("LANlord.lnk"), shortcut("gui")),
                file(programs.join("LANlord TUI.lnk"), shortcut("")),
            ]
        }
    })
}

/// a freedesktop entry: the window (`gui`), or the text interface in a terminal.
fn desktop_entry(exe: &Path, icon: &Path, tui: bool) -> String {
    let exe = exec_quote(&exe.to_string_lossy());
    let (name, exec, terminal, comment) = if tui {
        ("LANlord TUI", exe, "true", "LANlord in a terminal")
    } else {
        (
            "LANlord",
            format!("{exe} gui"),
            "false",
            "Manage your LANlord gateways",
        )
    };
    let mut entry = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment={comment}\nExec={exec}\n\
         Icon={}\nTerminal={terminal}\nCategories=Network;\n",
        escape_value(&icon.to_string_lossy())
    );
    if !tui {
        // matches the window's wayland app id, so the taskbar groups it under this entry
        entry.push_str("StartupWMClass=lanlord\n");
    }
    entry
}

/// one argument of an `Exec=` line, quoted when the desktop entry spec needs it.
fn exec_quote(arg: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(',
        ')', '`',
    ];
    let arg = arg.replace('%', "%%");
    let quoted = if arg.contains(RESERVED) {
        let mut out = String::from("\"");
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    } else {
        arg
    };
    escape_value(&quoted)
}

/// a string value in a desktop entry: backslashes and line breaks escaped.
fn escape_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

/// the script inside LANlord.app that opens the window from the installed binary.
fn mac_script(exe: &Path) -> String {
    let quoted = format!("'{}'", exe.to_string_lossy().replace('\'', "'\\''"));
    format!("#!/bin/sh\n# opens the lanlord desktop window\nexec {quoted} gui \"$@\"\n")
}

/// the same bundle description tools/build-app.sh writes.
fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>LANlord</string>
  <key>CFBundleDisplayName</key><string>LANlord</string>
  <key>CFBundleIdentifier</key><string>com.muncherelli.lanlord</string>
  <key>CFBundleExecutable</key><string>LANlord</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>{}</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#,
        env!("CARGO_PKG_VERSION")
    )
}

/// what the manifest remembers: files written, and folders created for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub files: BTreeSet<PathBuf>,
    pub dirs: BTreeSet<PathBuf>,
}

impl Manifest {
    fn parse(text: &str) -> Self {
        let mut manifest = Self::default();
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("f", path)) => {
                    manifest.files.insert(PathBuf::from(path));
                }
                Some(("d", path)) => {
                    manifest.dirs.insert(PathBuf::from(path));
                }
                _ => {}
            }
        }
        manifest
    }

    fn render(&self) -> String {
        let files = self.files.iter().map(|p| format!("f {}\n", p.display()));
        let dirs = self.dirs.iter().map(|p| format!("d {}\n", p.display()));
        files.chain(dirs).collect()
    }

    fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

/// `lanlord install-launcher`.
pub fn install(dry_run: bool, force: bool) -> Result<()> {
    let exe = std::env::current_exe().context("finding the running program")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let os = Os::current();
    let written = install_in(
        os,
        &Env::current(),
        &exe,
        dry_run,
        force,
        &mut std::io::stdout(),
    )?;
    if os == Os::Linux && !written.is_empty() {
        run_quietly(&cache_commands(&written));
    }
    Ok(())
}

/// install for `os` into the places `env` names, saying what happens on `out`; returns the
/// files written (none on a dry run).
pub fn install_in(
    os: Os,
    env: &Env,
    exe: &Path,
    dry_run: bool,
    force: bool,
    out: &mut dyn Write,
) -> Result<Vec<Planned>> {
    let files = plan(os, env, exe)?;
    let manifest = manifest_path(env)?;
    if dry_run {
        let known = Manifest::load(&manifest)?;
        for f in &files {
            if f.path.exists() && !known.files.contains(&f.path) && !force {
                writeln!(
                    out,
                    "would stop at {} (already there, not made by install-launcher; --force replaces it)",
                    f.path.display()
                )?;
            } else {
                writeln!(out, "would write {}", f.path.display())?;
            }
        }
        return Ok(Vec::new());
    }
    install_files(&files, &manifest, force)?;
    for f in &files {
        writeln!(out, "wrote {}", f.path.display())?;
    }
    Ok(files)
}

/// write `files`, refusing to replace any that the manifest at `manifest` doesn't list.
pub fn install_files(files: &[Planned], manifest: &Path, force: bool) -> Result<()> {
    let mut known = Manifest::load(manifest)?;
    let foreign: Vec<_> = files
        .iter()
        .filter(|f| f.path.exists() && !known.files.contains(&f.path))
        .map(|f| f.path.display().to_string())
        .collect();
    if !foreign.is_empty() && !force {
        bail!(
            "these already exist and weren't made by install-launcher: {}\n\
             run it again with --force to replace them",
            foreign.join(", ")
        );
    }
    for f in files {
        if let Some(parent) = f.path.parent() {
            create_dirs(parent, &mut known.dirs)?;
        }
        write(f)?;
        known.files.insert(f.path.clone());
    }
    if let Some(parent) = manifest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(manifest, known.render())
        .with_context(|| format!("writing {}", manifest.display()))
}

/// create `dir` and its missing parents, remembering the ones made here.
fn create_dirs(dir: &Path, made: &mut BTreeSet<PathBuf>) -> Result<()> {
    let missing: Vec<_> = dir
        .ancestors()
        .take_while(|d| !d.exists())
        .map(Path::to_path_buf)
        .collect();
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    made.extend(missing);
    Ok(())
}

fn write(f: &Planned) -> Result<()> {
    let context = || format!("writing {}", f.path.display());
    match &f.content {
        Content::Bytes(bytes) => std::fs::write(&f.path, bytes).with_context(context),
        Content::Script(text) => {
            std::fs::write(&f.path, text).with_context(context)?;
            make_executable(&f.path)
        }
        Content::Shortcut { target, args, icon } => write_shortcut(&f.path, target, args, icon),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // the launcher only ever runs as the user who installed it: owner only, nothing for others
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn write_shortcut(path: &Path, target: &Path, args: &str, icon: &Path) -> Result<()> {
    let mut link = mslnk::ShellLink::new(target).map_err(|e| anyhow!("{e}"))?;
    if !args.is_empty() {
        link.set_arguments(Some(args.into()));
    }
    link.set_icon_location(Some(icon.to_string_lossy().into_owned()));
    link.create_lnk(path)
        .map_err(|e| anyhow!("writing {}: {e}", path.display()))
}

#[cfg(not(windows))]
fn write_shortcut(path: &Path, _target: &Path, _args: &str, _icon: &Path) -> Result<()> {
    bail!(
        "{} is a windows shortcut and can only be made on windows",
        path.display()
    )
}

/// the commands that tell the desktop about new or removed entries and icons.
fn cache_commands(files: &[Planned]) -> Vec<(&'static str, Vec<OsString>)> {
    let dir_of = |name: &str| {
        files
            .iter()
            .find(|f| f.path.file_name().is_some_and(|n| n == name))
            .and_then(|f| f.path.parent().map(Path::to_path_buf))
    };
    let mut runs: Vec<(&str, Vec<OsString>)> = Vec::new();
    if let Some(apps) = dir_of("lanlord.desktop") {
        runs.push(("update-desktop-database", vec![apps.into()]));
    }
    if let Some(hicolor) = dir_of("lanlord.png")
        .as_deref()
        .and_then(|apps| apps.ancestors().nth(2))
    {
        runs.push((
            "gtk-update-icon-cache",
            vec!["-q".into(), "-t".into(), hicolor.into()],
        ));
    }
    runs.push(("kbuildsycoca6", vec![]));
    runs.push(("kbuildsycoca5", vec![]));
    runs
}

/// best effort: run each command, ignoring tools that aren't installed.
fn run_quietly(commands: &[(&str, Vec<OsString>)]) {
    for (program, args) in commands {
        let _ = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// `lanlord uninstall-launcher`.
pub fn uninstall(dry_run: bool) -> Result<()> {
    let removed = uninstall_in(&Env::current(), dry_run, &mut std::io::stdout())?;
    if Os::current() == Os::Linux && !removed.is_empty() {
        run_quietly(&cache_commands(&[]));
    }
    Ok(())
}

/// remove what install-launcher wrote under `env`, saying what happens on `out`; returns the
/// files removed (none on a dry run).
pub fn uninstall_in(env: &Env, dry_run: bool, out: &mut dyn Write) -> Result<Vec<PathBuf>> {
    let manifest = manifest_path(env)?;
    let known = Manifest::load(&manifest)?;
    if known.files.is_empty() {
        writeln!(
            out,
            "install-launcher hasn't put anything here; nothing to remove"
        )?;
        return Ok(Vec::new());
    }
    if dry_run {
        for f in &known.files {
            writeln!(out, "would remove {}", f.display())?;
        }
        return Ok(Vec::new());
    }
    let removed = uninstall_files(&manifest)?;
    for f in &removed {
        writeln!(out, "removed {}", f.display())?;
    }
    Ok(removed)
}

/// remove what the manifest at `manifest` lists, then the manifest; returns the files removed.
pub fn uninstall_files(manifest: &Path) -> Result<Vec<PathBuf>> {
    let known = Manifest::load(manifest)?;
    let mut removed = Vec::new();
    for f in &known.files {
        match std::fs::remove_file(f) {
            Ok(()) => removed.push(f.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", f.display())),
        }
    }
    // deepest first; a folder that has anything else in it stays
    for d in known.dirs.iter().rev() {
        let _ = std::fs::remove_dir(d);
    }
    std::fs::remove_file(manifest).with_context(|| format!("removing {}", manifest.display()))?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lanlord-launcher-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env_in(root: &Path) -> Env {
        Env {
            home: Some(root.join("home")),
            xdg_data_home: None,
            appdata: Some(root.join("appdata")),
            config_dir: Some(root.join("config")),
        }
    }

    fn paths(files: &[Planned]) -> Vec<PathBuf> {
        files.iter().map(|f| f.path.clone()).collect()
    }

    #[test]
    fn linux_puts_entries_and_icons_under_the_data_dir() {
        let env = env_in(Path::new("/r"));
        let files = plan(Os::Linux, &env, Path::new("/opt/lanlord")).unwrap();
        let share = Path::new("/r/home/.local/share");
        assert_eq!(
            paths(&files),
            vec![
                share.join("applications/lanlord.desktop"),
                share.join("applications/lanlord-tui.desktop"),
                share.join("icons/hicolor/512x512/apps/lanlord.png"),
                share.join("icons/hicolor/32x32/apps/lanlord.png"),
            ]
        );
        assert_eq!(files[2].content, Content::Bytes(ICON_PNG.to_vec()));
    }

    #[test]
    fn linux_honours_xdg_data_home() {
        let env = Env {
            xdg_data_home: Some("/data".into()),
            ..env_in(Path::new("/r"))
        };
        let files = plan(Os::Linux, &env, Path::new("/opt/lanlord")).unwrap();
        assert!(files.iter().all(|f| f.path.starts_with("/data")));
    }

    #[test]
    fn the_desktop_entries_open_the_window_and_the_terminal_ui() {
        let icon = Path::new("/i/lanlord.png");
        let gui = desktop_entry(Path::new("/opt/lanlord"), icon, false);
        assert!(gui.contains("\nName=LANlord\n"));
        assert!(gui.contains("\nExec=/opt/lanlord gui\n"));
        assert!(gui.contains("\nIcon=/i/lanlord.png\n"));
        assert!(gui.contains("\nTerminal=false\n"));
        assert!(gui.contains("\nStartupWMClass=lanlord\n"));
        let tui = desktop_entry(Path::new("/opt/lanlord"), icon, true);
        assert!(tui.contains("\nName=LANlord TUI\n"));
        assert!(tui.contains("\nExec=/opt/lanlord\n"));
        assert!(tui.contains("\nTerminal=true\n"));
        assert!(!tui.contains("StartupWMClass"));
    }

    #[test]
    fn exec_paths_with_spaces_and_specials_are_quoted() {
        assert_eq!(exec_quote("/opt/lanlord"), "/opt/lanlord");
        assert_eq!(exec_quote("/my apps/lanlord"), "\"/my apps/lanlord\"");
        assert_eq!(exec_quote("/a$b"), "\"/a\\\\$b\"");
        assert_eq!(exec_quote("/100%"), "/100%%");
    }

    #[test]
    fn mac_builds_an_app_in_the_home_applications_folder() {
        let env = env_in(Path::new("/r"));
        let files = plan(Os::Mac, &env, Path::new("/opt/it's/lanlord")).unwrap();
        let app = Path::new("/r/home/Applications/LANlord.app/Contents");
        assert_eq!(
            paths(&files),
            vec![
                app.join("Info.plist"),
                app.join("MacOS/LANlord"),
                app.join("Resources/icon.icns"),
            ]
        );
        let Content::Script(script) = &files[1].content else {
            panic!("the launcher is a script");
        };
        assert!(script.contains("exec '/opt/it'\\''s/lanlord' gui"));
        let Content::Bytes(plist) = &files[0].content else {
            panic!("the plist is bytes");
        };
        let plist = String::from_utf8(plist.clone()).unwrap();
        assert!(plist.contains("<key>CFBundleExecutable</key><string>LANlord</string>"));
    }

    #[test]
    fn windows_adds_start_menu_shortcuts_and_keeps_the_icon_in_the_config_dir() {
        let env = env_in(Path::new("/r"));
        let files = plan(Os::Windows, &env, Path::new("/opt/lanlord.exe")).unwrap();
        let programs = Path::new("/r/appdata/Microsoft/Windows/Start Menu/Programs/LANlord");
        assert_eq!(
            paths(&files),
            vec![
                PathBuf::from("/r/config/lanlord.ico"),
                programs.join("LANlord.lnk"),
                programs.join("LANlord TUI.lnk"),
            ]
        );
        assert_eq!(
            files[1].content,
            Content::Shortcut {
                target: "/opt/lanlord.exe".into(),
                args: "gui".into(),
                icon: "/r/config/lanlord.ico".into(),
            }
        );
    }

    #[test]
    fn missing_home_or_appdata_is_an_error() {
        let env = Env::default();
        assert!(plan(Os::Linux, &env, Path::new("/x")).is_err());
        assert!(plan(Os::Mac, &env, Path::new("/x")).is_err());
        assert!(plan(Os::Windows, &env, Path::new("/x")).is_err());
    }

    #[test]
    fn the_manifest_round_trips() {
        let mut m = Manifest::default();
        m.files.insert("/a/b.desktop".into());
        m.files.insert("/a/with space.png".into());
        m.dirs.insert("/a".into());
        assert_eq!(Manifest::parse(&m.render()), m);
    }

    #[test]
    fn install_then_uninstall_leaves_only_what_was_there() {
        let root = scratch("roundtrip");
        let env = env_in(&root);
        let files = plan(Os::Linux, &env, Path::new("/opt/lanlord")).unwrap();
        let manifest = manifest_path(&env).unwrap();
        // a file of the user's own in the same folder must survive
        let apps = root.join("home/.local/share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        let mine = apps.join("lanlord-preview.desktop");
        std::fs::write(&mine, "mine").unwrap();

        install_files(&files, &manifest, false).unwrap();
        assert!(files.iter().all(|f| f.path.exists()));
        // installing again replaces its own files without --force
        install_files(&files, &manifest, false).unwrap();

        let removed = uninstall_files(&manifest).unwrap();
        assert_eq!(removed.len(), files.len());
        assert!(files.iter().all(|f| !f.path.exists()));
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "mine");
        assert!(apps.exists(), "a folder with the user's files stays");
        assert!(
            !root.join("home/.local/share/icons").exists(),
            "folders it made go"
        );
        assert!(!manifest.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn install_refuses_to_replace_a_launcher_it_did_not_write() {
        let root = scratch("refuse");
        let env = env_in(&root);
        let files = plan(Os::Linux, &env, Path::new("/opt/lanlord")).unwrap();
        let manifest = manifest_path(&env).unwrap();
        let entry = &files[0].path;
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(entry, "hand made").unwrap();

        let err = install_files(&files, &manifest, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--force"), "{err}");
        assert_eq!(std::fs::read_to_string(entry).unwrap(), "hand made");
        assert!(!manifest.exists());

        install_files(&files, &manifest, true).unwrap();
        assert_ne!(std::fs::read_to_string(entry).unwrap(), "hand made");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn the_mac_launcher_script_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("script");
        let env = env_in(&root);
        let files = plan(Os::Mac, &env, Path::new("/opt/lanlord")).unwrap();
        install_files(&files, &manifest_path(&env).unwrap(), false).unwrap();
        let mode = std::fs::metadata(&files[1].path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "runs for its owner, nothing for anyone else"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uninstall_without_a_manifest_removes_nothing() {
        let root = scratch("empty");
        let env = env_in(&root);
        let manifest = manifest_path(&env).unwrap();
        assert!(Manifest::load(&manifest).unwrap().files.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// `base` with each part joined on, the way the plan builds its paths.
    fn under(base: &Path, parts: &[&str]) -> PathBuf {
        parts
            .iter()
            .fold(base.to_path_buf(), |p, part| p.join(part))
    }

    fn text(out: Vec<u8>) -> String {
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_dry_run_writes_nothing_and_says_what_would_stop_it() {
        let root = scratch("dry");
        let env = env_in(&root);
        // joined part by part, like the plan, so the printed path matches on windows too
        let entry = under(
            &root,
            &["home", ".local", "share", "applications", "lanlord.desktop"],
        );
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "hand made").unwrap();
        let exe = Path::new("/opt/lanlord");
        let mut out = Vec::new();
        let written = install_in(Os::Linux, &env, exe, true, false, &mut out).unwrap();
        assert!(written.is_empty());
        let said = text(out);
        assert!(
            said.contains(&format!("would stop at {}", entry.display())),
            "{said}"
        );
        assert!(said.contains("would write"), "{said}");
        assert!(!manifest_path(&env).unwrap().exists());
        // with --force the same file is just written
        let mut out = Vec::new();
        install_in(Os::Linux, &env, exe, true, true, &mut out).unwrap();
        assert!(!text(out).contains("would stop"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn install_and_uninstall_report_each_file() {
        let root = scratch("report");
        let env = env_in(&root);
        let mut out = Vec::new();
        let written = install_in(
            Os::Linux,
            &env,
            Path::new("/opt/lanlord"),
            false,
            false,
            &mut out,
        )
        .unwrap();
        assert_eq!(written.len(), 4);
        assert_eq!(text(out).matches("wrote ").count(), 4);

        let mut out = Vec::new();
        assert!(uninstall_in(&env, true, &mut out).unwrap().is_empty());
        assert_eq!(text(out).matches("would remove ").count(), 4);
        assert!(
            written.iter().all(|f| f.path.exists()),
            "a dry run removes nothing"
        );

        let mut out = Vec::new();
        assert_eq!(uninstall_in(&env, false, &mut out).unwrap().len(), 4);
        assert_eq!(text(out).matches("removed ").count(), 4);

        let mut out = Vec::new();
        assert!(uninstall_in(&env, false, &mut out).unwrap().is_empty());
        assert!(text(out).contains("nothing to remove"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn without_a_config_dir_there_is_no_manifest() {
        let env = Env {
            config_dir: None,
            ..env_in(Path::new("/r"))
        };
        assert!(manifest_path(&env).is_err());
        assert!(uninstall_in(&env, false, &mut Vec::new()).is_err());
        assert!(install_in(
            Os::Linux,
            &env,
            Path::new("/x"),
            true,
            false,
            &mut Vec::new()
        )
        .is_err());
    }

    #[test]
    fn an_unreadable_manifest_is_an_error() {
        let root = scratch("badmanifest");
        let env = env_in(&root);
        // a folder where the manifest file should be
        std::fs::create_dir_all(manifest_path(&env).unwrap()).unwrap();
        assert!(Manifest::load(&manifest_path(&env).unwrap()).is_err());
        assert!(uninstall_in(&env, false, &mut Vec::new()).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uninstall_stops_when_a_listed_file_cannot_be_removed() {
        let root = scratch("stuck");
        let manifest = root.join("launcher-files");
        // a non-empty folder where a listed file should be can't be removed as a file
        let stuck = root.join("stuck");
        std::fs::create_dir_all(stuck.join("inside")).unwrap();
        std::fs::write(&manifest, format!("f {}\n", stuck.display())).unwrap();
        let err = uninstall_files(&manifest).unwrap_err().to_string();
        assert!(err.contains("removing"), "{err}");
        assert!(
            manifest.exists(),
            "the manifest stays so a retry knows what is left"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_already_gone_are_skipped_on_uninstall() {
        let root = scratch("gone");
        let manifest = root.join("launcher-files");
        std::fs::write(
            &manifest,
            format!("f {}\n", root.join("never-there").display()),
        )
        .unwrap();
        assert!(uninstall_files(&manifest).unwrap().is_empty());
        assert!(!manifest.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_manifest_ignores_lines_it_does_not_know() {
        let m = Manifest::parse("f /a\nnonsense\nx /b\nd /c\n");
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.dirs.len(), 1);
    }

    #[test]
    fn the_desktop_caches_are_refreshed_for_the_folders_written() {
        let env = env_in(Path::new("/r"));
        let files = plan(Os::Linux, &env, Path::new("/opt/lanlord")).unwrap();
        let commands = cache_commands(&files);
        let share = under(Path::new("/r"), &["home", ".local", "share"]);
        assert_eq!(
            commands[0],
            (
                "update-desktop-database",
                vec![share.join("applications").into()]
            )
        );
        assert_eq!(
            commands[1],
            (
                "gtk-update-icon-cache",
                vec![
                    "-q".into(),
                    "-t".into(),
                    under(&share, &["icons", "hicolor"]).into()
                ]
            )
        );
        assert_eq!(
            cache_commands(&[]).len(),
            2,
            "only the kde rebuilds without files"
        );
        // a tool that isn't installed is no error
        run_quietly(&[("lanlord-no-such-tool", vec![])]);
    }

    #[cfg(not(windows))]
    #[test]
    fn windows_shortcuts_are_only_made_on_windows() {
        let root = scratch("lnk");
        let env = env_in(&root);
        let files = plan(Os::Windows, &env, Path::new("/opt/lanlord.exe")).unwrap();
        let err = install_files(&files, &manifest_path(&env).unwrap(), false).unwrap_err();
        assert!(err.to_string().contains("only be made on windows"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_running_platform_and_environment_are_found() {
        if cfg!(target_os = "linux") {
            assert_eq!(Os::current(), Os::Linux);
        }
        assert_eq!(Env::current().config_dir, lanlord_core::paths::config_dir());
    }
}
