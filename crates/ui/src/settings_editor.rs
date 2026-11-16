//! the settings window: the plain settings as yaml text, and the secrets as
//! write-only fields. nothing is stored or applied unless a button is clicked.

use eframe::egui::{self, ScrollArea, TextEdit, Ui};
use lanlord_core::modules::{Status, MODULES};
use lanlord_core::settings::{secret_fields, Document, SecretField, Settings};

use crate::controls;
use crate::dialogs::{Answer, TextDialog};
use crate::services::Services;
use crate::tasks::Job;
use crate::theme::{self, WHITE};

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

#[derive(Default)]
pub struct SettingsEditor {
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
    /// the box asking for a secret's new value.
    secret_dialog: Option<(String, TextDialog)>,
}

impl SettingsEditor {
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

    /// ask for a new value of the secret at `path`.
    pub fn ask_secret(&mut self, path: &str) {
        let prompt = format!("New value for {path}\n(blank keeps the current one):");
        self.secret_dialog = Some((
            path.to_string(),
            TextDialog::new("secret", "Set a secret", &prompt, true),
        ));
    }

    #[cfg(test)]
    pub fn secret_dialog_open(&self) -> bool {
        self.secret_dialog.is_some()
    }

    /// remember a typed secret until Save; a blank one cancels a pending change.
    fn set_update(&mut self, path: String, value: String) {
        self.updates.retain(|(p, _)| *p != path);
        if !value.is_empty() {
            self.updates.push((path, value));
        }
    }

    /// the masked box for a secret, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some((path, mut dialog)) = self.secret_dialog.take() else {
            return;
        };
        match dialog.show(ctx) {
            Some(Answer::Ok(value)) => self.set_update(path, value),
            Some(Answer::Cancel) => {}
            None => self.secret_dialog = Some((path, dialog)),
        }
    }

    /// draw the window's contents for the selected `host`.
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let Some(host) = host else {
            ui.label("No host yet. Add one in the Hosts window first.");
            return;
        };
        if self.wants_load(Some(host)) {
            self.load(services, host);
        }
        let idle = !services.busy(host) && !self.working();
        let stale = !self.belongs_to(host);
        let problem = self.problem();
        controls::group(ui, &format!("Plain settings stored on {host}"), |ui| {
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt("settings-text")
                    .max_height(EDITOR_HEIGHT)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), EDITOR_HEIGHT],
                            TextEdit::multiline(&mut self.text)
                                .font(egui::TextStyle::Monospace)
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                    });
            });
        });
        ui.add_space(4.0);
        controls::group(ui, "Secrets (write-only; values are never shown)", |ui| {
            self.secrets_list(ui, idle)
        });
        ui.add_space(6.0);
        self.buttons(
            ui,
            services,
            host,
            idle && problem.is_none() && !stale,
            idle,
        );
        ui.add_space(6.0);
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
        controls::status_bar(ui, &text, error);
    }

    fn secrets_list(&mut self, ui: &mut Ui, idle: bool) {
        let fields = self.fields();
        let mut ask = None;
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt("settings-secrets")
                .max_height(96.0)
                .show(ui, |ui| {
                    if fields.is_empty() {
                        ui.label("None found in the settings.");
                    }
                    for field in &fields {
                        ui.horizontal(|ui| {
                            let state = match (self.pending_update(&field.path), field.set) {
                                (true, _) => "new value on Save",
                                (false, true) => "set",
                                (false, false) => "not set",
                            };
                            ui.label(format!("{}: {state}", field.path));
                            if theme::action(ui, "Set...", idle).clicked() {
                                ask = Some(field.path.clone());
                            }
                        });
                    }
                });
        });
        if let Some(path) = ask {
            self.ask_secret(&path);
        }
    }

    fn buttons(
        &mut self,
        ui: &mut Ui,
        services: &Services,
        host: &str,
        can_save: bool,
        idle: bool,
    ) {
        ui.horizontal(|ui| {
            if theme::action(ui, "Save", can_save).clicked() {
                self.save(services, host);
            }
            if theme::action(ui, "Reload", idle).clicked() {
                self.load(services, host);
            }
            if theme::action(ui, "Apply", idle).clicked() {
                self.apply(services, host, &[]);
            }
            ui.add_space(12.0);
            let modules: Vec<_> = MODULES
                .iter()
                .enumerate()
                .filter(|(_, m)| m.status == Status::Available)
                .collect();
            let name = MODULES.get(self.module).map_or("-", |m| m.name);
            controls::dropdown(ui, "settings-module", name, 120.0, |ui| {
                for (i, module) in modules {
                    ui.selectable_value(&mut self.module, i, module.name);
                }
            });
            if theme::action(ui, "Apply module", idle).clicked() {
                let id = MODULES[self.module].id.to_string();
                self.apply(services, host, &[id]);
            }
        });
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

    fn setup(stored: &str) -> (SettingsEditor, Services, Arc<Fake>) {
        let fake = Arc::new(Fake::new());
        *fake.stored.lock().unwrap() = stored.to_string();
        let services = Services::new(fake.clone(), &egui::Context::default());
        (SettingsEditor::default(), services, fake)
    }

    fn settle(editor: &mut SettingsEditor) {
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
    fn the_secret_dialog_collects_a_value() {
        let (mut editor, _, _) = setup("");
        let ctx = egui::Context::default();
        let frame = |editor: &mut SettingsEditor, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                theme::apply(ui.ctx());
                editor.dialog(ui.ctx());
            })
            .textures_delta
            .clear();
        };
        frame(&mut editor, Vec::new());
        editor.ask_secret("a.password");
        frame(&mut editor, Vec::new());
        frame(&mut editor, Vec::new());
        frame(&mut editor, vec![egui::Event::Text("s3".into())]);
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut editor, vec![enter]);
        assert!(editor.pending_update("a.password"));
        assert!(editor.secret_dialog.is_none());
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
}
