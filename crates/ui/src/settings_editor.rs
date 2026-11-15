//! the settings window: your decrypted settings as yaml text. nothing is
//! stored or applied unless a button is clicked.

use eframe::egui::{self, Color32, ComboBox, RichText, ScrollArea, TextEdit, Ui};
use lanlord_core::modules::{Status, MODULES};
use lanlord_core::settings::Settings;

use crate::services::Services;
use crate::tasks::Job;
use crate::theme::{self, WHITE};

enum Pending {
    Load(String, Job<String>),
    Save(String, String, Job<()>),
}

/// a line under the editor: true when it is an error.
type Message = (bool, String);

#[derive(Default)]
pub struct SettingsEditor {
    pub text: String,
    /// the text as it was last loaded or stored; differs once you edit.
    clean: String,
    /// the host `text` came from.
    source: Option<String>,
    /// a host whose load failed, so it is not retried every frame.
    failed: Option<String>,
    pending: Option<Pending>,
    pub message: Option<Message>,
    /// the module the picker's Apply button targets.
    pub module: usize,
}

impl SettingsEditor {
    /// what is wrong with the yaml, if anything.
    pub fn parse_error(&self) -> Option<String> {
        Settings::parse(&self.text).err().map(|e| e.to_string())
    }

    pub fn dirty(&self) -> bool {
        self.text != self.clean
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

    fn say(&mut self, error: bool, text: impl Into<String>) {
        self.message = Some((error, text.into()));
    }

    /// fetch and decrypt the settings of `host`.
    pub fn load(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.load_settings_text(&target, ask)
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

    /// store exactly the text in the editor, once it parses.
    pub fn save(&mut self, services: &Services, host: &str) {
        if let Some(error) = self.parse_error() {
            self.say(true, format!("not saved: {error}"));
            return;
        }
        let (target, text) = (host.to_string(), self.text.clone());
        let sent = text.clone();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.save_settings_text(&target, &sent, ask)
        });
        match job {
            Some(job) => {
                self.say(false, "saving...");
                self.pending = Some(Pending::Save(host.to_string(), text, job));
            }
            None => self.say(true, "the gateway is busy"),
        }
    }

    /// pick up the result of a load or save that finished.
    pub fn poll(&mut self) {
        let finished = match &self.pending {
            Some(Pending::Load(host, job)) => job.poll().map(|r| (host.clone(), None, r)),
            Some(Pending::Save(host, text, job)) => job
                .poll()
                .map(|r| (host.clone(), Some(text.clone()), r.map(|()| String::new()))),
            None => None,
        };
        let Some((host, saved, result)) = finished else {
            return;
        };
        self.pending = None;
        match (saved, result) {
            (None, Ok(text)) => {
                self.clean.clone_from(&text);
                self.text = text;
                self.source = Some(host.clone());
                self.say(false, format!("loaded from {host}"));
            }
            (Some(text), Ok(_)) => {
                self.clean = text;
                self.source = Some(host.clone());
                self.say(false, format!("saved to {host}"));
            }
            (None, Err(err)) => {
                self.failed = Some(host);
                self.say(true, err);
            }
            (Some(_), Err(err)) => self.say(true, err),
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
        ui.label(format!("Settings stored on {host}"));
        let parse_error = self.parse_error();
        let editor_height = (ui.available_height() - 84.0).max(60.0);
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .max_height(editor_height)
                .show(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), editor_height],
                        TextEdit::multiline(&mut self.text)
                            .font(egui::TextStyle::Monospace)
                            .frame(false)
                            .desired_width(f32::INFINITY),
                    );
                });
        });
        match (&parse_error, &self.message) {
            (Some(error), _) => {
                ui.colored_label(Color32::from_rgb(160, 0, 0), format!("yaml: {error}"));
            }
            (None, Some((true, text))) => {
                ui.colored_label(Color32::from_rgb(160, 0, 0), text);
            }
            (None, Some((false, text))) => {
                ui.label(text);
            }
            (None, None) => {
                ui.label("");
            }
        }
        if services.busy(host) {
            ui.label(RichText::new("working...").italics());
        } else if stale && self.source.is_some() {
            ui.label("The text is from another host; Reload to switch.");
        }
        ui.horizontal(|ui| {
            let can_save = idle && parse_error.is_none() && !stale;
            if theme::action(ui, "Save", can_save).clicked() {
                self.save(services, host);
            }
            if theme::action(ui, "Reload", idle).clicked() {
                self.load(services, host);
            }
            if theme::action(ui, "Apply", idle).clicked() {
                self.apply(services, host, &[]);
            }
        });
        ui.horizontal(|ui| {
            let modules: Vec<_> = MODULES
                .iter()
                .enumerate()
                .filter(|(_, m)| m.status == Status::Available)
                .collect();
            let name = MODULES.get(self.module).map_or("-", |m| m.name);
            ComboBox::from_id_salt("settings-module")
                .selected_text(name)
                .show_ui(ui, |ui| {
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
        assert!(editor.parse_error().is_some());
        editor.save(&services, "a@b");
        assert!(!editor.working());
        assert!(fake.calls().is_empty());
        assert!(editor.message.as_ref().unwrap().1.starts_with("not saved"));
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
