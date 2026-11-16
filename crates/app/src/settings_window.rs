//! the Settings window: the plain settings as yaml text, and the secrets as write-only
//! fields. nothing is stored or applied unless a button is clicked.

use lanlord_core::modules::{Status, MODULES};
use lanlord_core::settings::{secret_fields, Document, SecretField, Settings};

use zeroize::Zeroizing;

use crate::services::Services;
use crate::tasks::Job;
use crate::view::{Button, Node};

/// the height of the settings text box.
const EDITOR_HEIGHT: f32 = 230.0;

/// a secret value typed in the dialog, waiting for Save: (path, value).
type Update = (String, String);

enum Pending {
    Load(String, Job<Document>),
    Save(String, String, Vec<Update>, Job<()>),
}

/// a line under the editor: true when it is an error.
type Message = (bool, String);

/// what a click or a keystroke in the window means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// the settings text now reads this.
    Edit(String),
    /// ask for a new value of the secret at this path.
    SetSecret(String),
    Save,
    Reload,
    Apply,
    /// the module picker now shows the module at this index of `MODULES`.
    PickModule(usize),
    ApplyModule,
}

#[derive(Default)]
pub struct SettingsWindow {
    /// the plain settings text.
    pub text: String,
    /// the text as it was last loaded or stored; differs once you edit.
    clean: String,
    /// the paths of the secrets that have a value on the gateway.
    secrets_set: Vec<String>,
    /// secrets typed since the last save; blank means unchanged and is not kept.
    updates: Vec<Update>,
    /// the gateway still has everything in the old single encrypted file.
    legacy: bool,
    /// the host `text` came from.
    source: Option<String>,
    /// a host whose load failed, so it is not retried every frame.
    failed: Option<String>,
    pending: Option<Pending>,
    pub message: Option<Message>,
    /// the module the picker's Apply button targets.
    pub module: usize,
    /// the secret whose new value the front end is asking for.
    asking: Option<String>,
}

impl SettingsWindow {
    /// what is wrong with the text: bad yaml, or a secret in the plain settings.
    pub fn problem(&self) -> Option<String> {
        match Settings::parse(&self.text) {
            Err(error) => Some(error.to_string()),
            Ok(settings) => {
                let leaked = settings.secret_paths();
                (!leaked.is_empty()).then(|| {
                    format!(
                        "secrets do not belong in the plain settings ({}); use the secrets list",
                        leaked.join(", ")
                    )
                })
            }
        }
    }

    pub fn dirty(&self) -> bool {
        self.text != self.clean || !self.updates.is_empty()
    }

    pub fn working(&self) -> bool {
        self.pending.is_some()
    }

    /// should the editor fetch `host`'s settings now? yes when it shows
    /// another host's text that you have not edited.
    pub fn wants_load(&self, host: Option<&str>) -> bool {
        host.is_some()
            && self.source.as_deref() != host
            && self.failed.as_deref() != host
            && !self.dirty()
            && self.pending.is_none()
    }

    /// is the text the one stored on `host`, as far as we know?
    pub fn belongs_to(&self, host: &str) -> bool {
        self.source.as_deref() == Some(host)
    }

    /// the secrets to list, each with whether it has a value.
    pub fn fields(&self) -> Vec<SecretField> {
        secret_fields(&self.text, &self.secrets_set)
    }

    /// is a new value for `path` waiting for Save?
    pub fn pending_update(&self, path: &str) -> bool {
        self.updates.iter().any(|(p, _)| p == path)
    }

    fn say(&mut self, error: bool, text: impl Into<String>) {
        self.message = Some((error, text.into()));
    }

    /// fetch the settings of `host`.
    pub fn load(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.load_document(&target, ask)
        });
        match job {
            Some(job) => {
                self.failed = None;
                self.say(false, "loading...");
                self.pending = Some(Pending::Load(host.to_string(), job));
            }
            None => self.say(true, "the gateway is busy"),
        }
    }

    /// store exactly the text in the editor, once it is sound, and the typed secrets.
    pub fn save(&mut self, services: &Services, host: &str) {
        if let Some(problem) = self.problem() {
            self.say(true, format!("not saved: {problem}"));
            return;
        }
        let (target, text, updates) = (host.to_string(), self.text.clone(), self.updates.clone());
        let (sent, sent_updates) = (text.clone(), updates.clone());
        let job = services.spawn(host, move |gateway, ask| {
            gateway.save_document(&target, &sent, &sent_updates, ask)
        });
        match job {
            Some(job) => {
                self.say(false, "saving...");
                self.pending = Some(Pending::Save(host.to_string(), text, updates, job));
            }
            None => self.say(true, "the gateway is busy"),
        }
    }

    /// pick up the result of a load or save that finished.
    pub fn poll(&mut self) {
        match self.pending.take() {
            Some(Pending::Load(host, job)) => match job.poll() {
                None => self.pending = Some(Pending::Load(host, job)),
                Some(Ok(document)) => self.loaded(host, document),
                Some(Err(error)) => {
                    self.failed = Some(host);
                    self.say(true, error);
                }
            },
            Some(Pending::Save(host, text, updates, job)) => match job.poll() {
                None => self.pending = Some(Pending::Save(host, text, updates, job)),
                Some(Ok(())) => self.saved(host, text, updates),
                Some(Err(error)) => self.say(true, error),
            },
            None => {}
        }
    }

    fn loaded(&mut self, host: String, document: Document) {
        self.clean.clone_from(&document.plain);
        self.text = document.plain;
        self.secrets_set = document.secrets_set;
        self.legacy = document.legacy;
        self.updates.clear();
        self.say(false, format!("loaded from {host}"));
        self.source = Some(host);
    }

    fn saved(&mut self, host: String, text: String, updates: Vec<Update>) {
        self.clean = text;
        for (path, value) in updates {
            self.updates.retain(|(p, _)| *p != path);
            if !value.is_empty() && !self.secrets_set.contains(&path) {
                self.secrets_set.push(path);
            }
        }
        self.legacy = false;
        self.say(false, format!("saved to {host}"));
        self.source = Some(host);
    }

    /// ask the front end for a new value of the secret at `path`.
    pub fn ask_secret(&mut self, path: &str) {
        self.asking = Some(path.to_string());
    }

    /// the secret whose new value is being asked for, if any.
    pub fn asking(&self) -> Option<&str> {
        self.asking.as_deref()
    }

    /// the answer to `asking`: the typed value, or `None` when it was cancelled.
    pub fn answer_secret(&mut self, value: Option<Zeroizing<String>>) {
        if let (Some(path), Some(value)) = (self.asking.take(), value) {
            // kept until Save with the other typed secrets
            self.set_update(path, value.to_string());
        }
    }

    /// remember a typed secret until Save; a blank one cancels a pending change.
    pub fn set_update(&mut self, path: String, value: String) {
        self.updates.retain(|(p, _)| *p != path);
        if !value.is_empty() {
            self.updates.push((path, value));
        }
    }

    /// nothing is running for `host`, here or on the gateway.
    pub fn idle(&self, services: &Services, host: &str) -> bool {
        !services.busy(host) && !self.working()
    }

    /// Save is allowed: idle, sound text, and the text is `host`'s own.
    pub fn can_save(&self, services: &Services, host: &str) -> bool {
        self.idle(services, host) && self.problem().is_none() && self.belongs_to(host)
    }

    /// the window for the selected `host`; starts a load when it shows another host's text.
    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No host yet. Add one in the Hosts window first.".into(),
            )];
        };
        if self.wants_load(Some(host)) {
            self.load(services, host);
        }
        let idle = self.idle(services, host);
        let stale = !self.belongs_to(host);
        let problem = self.problem();
        let can_save = self.can_save(services, host);
        let fields = self.fields();
        let mut secrets: Vec<Node<Msg>> = fields
            .iter()
            .map(|field| {
                let state = match (self.pending_update(&field.path), field.set) {
                    (true, _) => "new value on Save",
                    (false, true) => "set",
                    (false, false) => "not set",
                };
                Node::Row(vec![
                    Node::Text(format!("{}: {state}", field.path)),
                    Node::Button(Button::new(
                        "Set...",
                        idle,
                        Msg::SetSecret(field.path.clone()),
                    )),
                ])
            })
            .collect();
        if secrets.is_empty() {
            secrets.push(Node::Text("None found in the settings.".into()));
        }
        let modules = MODULES
            .iter()
            .enumerate()
            .filter(|(_, m)| m.status == Status::Available)
            .map(|(i, m)| (i, m.name.to_string()))
            .collect();
        let (error, text) = match (&problem, &self.message) {
            (Some(problem), _) => (true, problem.clone()),
            (None, Some((error, text))) => (*error, text.clone()),
            (None, None) if services.busy(host) => (false, "working...".to_string()),
            (None, None) if stale && self.source.is_some() => (
                false,
                "The text is from another host; Reload to switch.".to_string(),
            ),
            (None, None) if self.legacy => (
                false,
                "Stored the old way; Save moves the secrets to their own file.".to_string(),
            ),
            (None, None) => (false, String::new()),
        };
        vec![
            Node::Group {
                title: format!("Plain settings stored on {host}"),
                body: vec![Node::TextArea {
                    id: "settings-text",
                    value: self.text.clone(),
                    height: EDITOR_HEIGHT,
                    hint: String::new(),
                    edit: Msg::Edit,
                }],
            },
            Node::Gap(4.0),
            Node::Group {
                title: "Secrets (write-only; values are never shown)".into(),
                body: vec![Node::Scroll {
                    id: "settings-secrets",
                    max_height: 96.0,
                    body: secrets,
                }],
            },
            Node::Gap(6.0),
            Node::Row(vec![
                Node::Button(Button::new("Save", can_save, Msg::Save)),
                Node::Button(Button::new("Reload", idle, Msg::Reload)),
                Node::Button(Button::new("Apply", idle, Msg::Apply)),
                Node::Gap(12.0),
                Node::Choice {
                    id: "settings-module",
                    current: MODULES.get(self.module).map_or("-", |m| m.name).into(),
                    width: 120.0,
                    options: modules,
                    pick: Msg::PickModule,
                },
                Node::Button(Button::new("Apply module", idle, Msg::ApplyModule)),
            ]),
            Node::Gap(6.0),
            Node::Status { text, error },
        ]
    }

    /// act on a click or keystroke for `host`.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        // the same rules as the buttons: a front end may send any message
        let idle = self.idle(services, host);
        match msg {
            Msg::Edit(text) => self.text = text,
            Msg::SetSecret(path) if idle => self.ask_secret(&path),
            Msg::Save if self.can_save(services, host) => self.save(services, host),
            Msg::Reload if idle => self.load(services, host),
            Msg::Apply if idle => self.apply(services, host, &[]),
            Msg::PickModule(i) => self.module = i,
            Msg::ApplyModule if idle => {
                let id = MODULES[self.module].id.to_string();
                self.apply(services, host, &[id]);
            }
            Msg::SetSecret(_) | Msg::Save | Msg::Reload | Msg::Apply | Msg::ApplyModule => {}
        }
    }

    /// apply everything (or one module) to `host`.
    pub fn apply(&mut self, services: &Services, host: &str, modules: &[String]) {
        if services.apply(host, modules.to_vec()) {
            self.say(false, "applying; watch the console");
        } else {
            self.say(true, "the gateway is busy");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    fn setup(stored: &str) -> (SettingsWindow, Services, Arc<Fake>) {
        let fake = Arc::new(Fake::new());
        *fake.stored.lock().unwrap() = stored.to_string();
        let services = Services::new(fake.clone(), Wake::none());
        (SettingsWindow::default(), services, fake)
    }

    fn settle(editor: &mut SettingsWindow) {
        let start = Instant::now();
        while editor.working() {
            editor.poll();
            assert!(start.elapsed() < Duration::from_secs(5), "never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loading_fills_the_editor_and_remembers_the_host() {
        let (mut editor, services, fake) = setup("# hi\nmotd: x\n");
        assert!(editor.wants_load(Some("a@b")) && !editor.wants_load(None));
        editor.load(&services, "a@b");
        assert!(editor.working());
        settle(&mut editor);
        assert_eq!(editor.text, "# hi\nmotd: x\n");
        assert!(!editor.dirty() && editor.belongs_to("a@b"));
        assert!(!editor.wants_load(Some("a@b")));
        assert!(editor.wants_load(Some("c@d")), "another host reloads");
        assert_eq!(fake.calls(), ["load a@b"]);
    }

    #[test]
    fn editing_blocks_the_automatic_reload() {
        let (mut editor, services, _) = setup("a: 1\n");
        editor.load(&services, "a@b");
        settle(&mut editor);
        editor.text.push_str("b: 2\n");
        assert!(editor.dirty());
        assert!(
            !editor.wants_load(Some("c@d")),
            "edits are never thrown away"
        );
    }

    #[test]
    fn a_failed_load_is_shown_and_not_retried() {
        let (mut editor, services, fake) = setup("");
        *fake.fail.lock().unwrap() = Some("no key yet".into());
        editor.load(&services, "a@b");
        settle(&mut editor);
        assert_eq!(editor.message, Some((true, "no key yet".to_string())));
        assert!(!editor.wants_load(Some("a@b")));
        editor.load(&services, "a@b");
        settle(&mut editor);
        assert_eq!(fake.calls().len(), 2, "an explicit reload tries again");
    }

    #[test]
    fn saving_pushes_exactly_what_was_typed() {
        let (mut editor, services, fake) = setup("a: 1\n");
        editor.load(&services, "a@b");
        settle(&mut editor);
        editor.text = "# keep this comment\na: 2\n".into();
        editor.save(&services, "a@b");
        settle(&mut editor);
        assert_eq!(*fake.stored.lock().unwrap(), "# keep this comment\na: 2\n");
        assert!(!editor.dirty());
        assert_eq!(editor.message, Some((false, "saved to a@b".to_string())));
    }

    #[test]
    fn broken_yaml_is_never_sent() {
        let (mut editor, services, fake) = setup("");
        editor.text = "a: [oops\n".into();
        assert!(editor.problem().is_some());
        editor.save(&services, "a@b");
        assert!(!editor.working());
        assert!(fake.calls().is_empty());
        assert!(editor.message.as_ref().unwrap().1.starts_with("not saved"));
    }

    #[test]
    fn a_secret_in_the_plain_text_is_refused_with_a_message() {
        let (mut editor, services, fake) = setup("");
        editor.text = "nt4-domain:\n  admin_password: oops\n".into();
        let problem = editor.problem().unwrap();
        assert!(problem.contains("nt4-domain.admin_password"), "{problem}");
        editor.save(&services, "a@b");
        assert!(!editor.working() && fake.calls().is_empty());
    }

    #[test]
    fn a_failed_save_keeps_the_edits() {
        let (mut editor, services, fake) = setup("");
        *fake.fail.lock().unwrap() = Some("denied".into());
        editor.text = "a: 1\n".into();
        editor.save(&services, "a@b");
        settle(&mut editor);
        assert!(editor.dirty());
        assert_eq!(editor.message, Some((true, "denied".to_string())));
    }

    #[test]
    fn secrets_are_listed_set_through_a_dialog_and_saved_write_only() {
        let (mut editor, services, fake) = setup("nt4-domain:\n  users: [{name: dana}]\n");
        fake.secrets
            .lock()
            .unwrap()
            .push(("nt4-domain.admin_password".into(), "old".into()));
        editor.load(&services, "a@b");
        settle(&mut editor);
        let fields = editor.fields();
        assert_eq!(fields.len(), 2);
        assert!(fields[0].set && !fields[1].set);

        editor.set_update("nt4-domain.users[dana].password".into(), "pw".into());
        assert!(editor.pending_update("nt4-domain.users[dana].password"));
        assert!(editor.dirty());
        editor.set_update("nt4-domain.users[dana].password".into(), String::new());
        assert!(
            !editor.pending_update("nt4-domain.users[dana].password"),
            "blank cancels"
        );
        editor.set_update("nt4-domain.users[dana].password".into(), "pw".into());
        editor.set_update("nt4-domain.admin_password".into(), String::new());

        editor.save(&services, "a@b");
        settle(&mut editor);
        let stored = fake.secrets.lock().unwrap().clone();
        assert!(stored.contains(&(
            "nt4-domain.users[dana].password".to_string(),
            "pw".to_string()
        )));
        assert!(stored.contains(&("nt4-domain.admin_password".to_string(), "old".to_string())));
        assert!(!editor.dirty());
        assert!(editor.fields().iter().all(|f| f.set));
    }

    #[test]
    fn a_secret_is_asked_for_and_kept_until_save() {
        let (mut editor, _, _) = setup("");
        editor.ask_secret("a.password");
        assert_eq!(editor.asking(), Some("a.password"));
        editor.answer_secret(Some(Zeroizing::new("s3".into())));
        assert!(editor.pending_update("a.password"));
        assert_eq!(editor.asking(), None);
        editor.ask_secret("b.password");
        editor.answer_secret(None);
        assert!(!editor.pending_update("b.password"), "cancel keeps nothing");
    }

    #[test]
    fn the_buttons_and_the_text_drive_the_window() {
        let (mut editor, services, fake) = setup("a: 1\n");
        editor.view(&services, Some("a@b"));
        settle(&mut editor);
        editor.update(Msg::Edit("a: 2\n".into()), &services, "a@b");
        assert!(editor.dirty());
        editor.update(Msg::Save, &services, "a@b");
        settle(&mut editor);
        assert_eq!(*fake.stored.lock().unwrap(), "a: 2\n");
        editor.update(Msg::SetSecret("x.password".into()), &services, "a@b");
        assert_eq!(editor.asking(), Some("x.password"));
    }

    #[test]
    fn saving_stores_nothing_new_in_the_editor_until_the_gateway_agrees() {
        let (mut editor, services, fake) = setup("");
        *fake.fail.lock().unwrap() = Some("denied".into());
        editor.set_update("a.password".into(), "x".into());
        editor.save(&services, "a@b");
        settle(&mut editor);
        assert!(editor.pending_update("a.password"), "kept for another try");
        assert!(!editor.fields().iter().any(|f| f.set));
    }

    #[test]
    fn a_busy_host_refuses_a_second_task() {
        let (mut editor, services, fake) = setup("");
        *fake.password_prompt.lock().unwrap() = Some("pw".into());
        assert!(services.apply("a@b", Vec::new()));
        editor.load(&services, "a@b");
        assert!(!editor.working());
        assert_eq!(
            editor.message,
            Some((true, "the gateway is busy".to_string()))
        );
        editor.save(&services, "a@b");
        assert!(!editor.working());
        editor.apply(&services, "a@b", &[]);
        assert!(editor.message.as_ref().unwrap().0);
        // let the waiting task finish
        let request = loop {
            if let Some(request) = services.tasks.next_request() {
                break request;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        request.reply.send(None).unwrap();
    }

    #[test]
    fn save_refuses_another_hosts_text_and_a_running_task_even_without_the_button() {
        let (mut editor, services, fake) = setup("a: 1\n");
        editor.load(&services, "a@b");
        settle(&mut editor);
        // the text belongs to a@b: saving it to c@d is refused
        editor.update(Msg::Save, &services, "c@d");
        assert!(!editor.working());
        // while a load runs, nothing else starts
        editor.load(&services, "a@b");
        editor.update(Msg::Save, &services, "a@b");
        editor.update(Msg::SetSecret("x.password".into()), &services, "a@b");
        assert_eq!(editor.asking(), None);
        settle(&mut editor);
        assert_eq!(fake.calls(), ["load a@b", "load a@b"]);
    }
}
