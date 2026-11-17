//! what every window needs to reach a gateway: the gateway itself (real or
//! fake), the task runner and the front end to wake up.

use std::sync::Arc;

use anyhow::Result;
use lanlord_model::power::{Outcome, Power};
use lanlord_model::settings::Settings;

use crate::ask::{cached, AskPassword};
use crate::compat::Compat;
use crate::log::{emit, Kind};

use crate::gateway::Gateway;
use crate::tasks::{Job, Tasks};
use crate::wake::Wake;

pub struct Services {
    gateway: Arc<dyn Gateway>,
    tasks: Tasks,
    wake: Wake,
    compat: Compat,
}

impl Services {
    pub fn new(gateway: Arc<dyn Gateway>, wake: Wake) -> Self {
        Self {
            gateway,
            tasks: Tasks::default(),
            wake,
            compat: Compat::default(),
        }
    }

    /// what the server can do for the components, shared by the windows that show it.
    pub fn compat(&self) -> &Compat {
        &self.compat
    }

    /// the next password a task is waiting for, if any.
    pub fn next_request(&self) -> Option<crate::tasks::PasswordRequest> {
        self.tasks.next_request()
    }

    /// is a task running against `host`?
    pub fn busy(&self, host: &str) -> bool {
        self.tasks.busy(host)
    }

    /// run `work` in the background with the gateway; `None` when `host` already has a task.
    /// the sudo password is asked for once per task, however many steps need it.
    pub fn spawn<T: Send + 'static>(
        &self,
        host: &str,
        work: impl Fn(&dyn Gateway, AskPassword) -> Result<T> + Send + Sync + 'static,
    ) -> Option<Job<T>> {
        #[cfg(target_arch = "wasm32")]
        if let Some(http) = self.gateway.http() {
            return self.tasks.spawn_web(&self.wake, host, http.clone(), work);
        }
        let gateway = self.gateway.clone();
        self.tasks.spawn(&self.wake, host, move |ask| {
            let mut ask = cached(ask);
            work(&*gateway, &mut ask)
        })
    }

    /// run a small read with the gateway in the background, without marking any host busy.
    pub fn quiet<T: Send + 'static>(
        &self,
        work: impl Fn(&dyn Gateway) -> Result<T> + Send + Sync + 'static,
    ) -> Job<T> {
        #[cfg(target_arch = "wasm32")]
        if let Some(http) = self.gateway.http() {
            return self.tasks.quiet_web(&self.wake, http.clone(), work);
        }
        let gateway = self.gateway.clone();
        self.tasks.quiet(&self.wake, move || work(&*gateway))
    }

    /// bootstrap `host` in the background; progress goes to the console.
    pub fn bootstrap(&self, host: &str) -> bool {
        let target = host.to_string();
        self.spawn(host, move |gateway, ask| gateway.bootstrap(&target, ask))
            .is_some()
    }

    /// has the server said this login is over? the desktop shows its logon box again.
    pub fn session_ended(&self) -> bool {
        self.gateway.session_ended()
    }

    /// the person signed in again: stop reporting the session as ended.
    pub fn session_restored(&self) {
        self.gateway.session_restored()
    }

    /// is lanlord running on the gateway itself?
    pub fn is_local(&self) -> bool {
        self.gateway.is_local()
    }

    /// reboot or power off `host` in the background. the connection dropping
    /// is what success looks like, so it is reported as the machine going down.
    pub fn power(&self, host: &str, action: Power) -> bool {
        let target = host.to_string();
        self.spawn(host, move |gateway, ask| {
            let outcome = gateway.power(&target, action, ask)?;
            let note = match outcome {
                Outcome::Done => format!("{target}: {} now", action.doing()),
                Outcome::Dropped => format!(
                    "{target}: {}; the connection dropped, as expected",
                    action.doing()
                ),
            };
            emit(Kind::Info, note);
            Ok(())
        })
        .is_some()
    }

    /// apply `modules` (all when empty) to `host` in the background.
    pub fn apply(&self, host: &str, modules: Vec<String>) -> bool {
        let target = host.to_string();
        self.spawn(host, move |gateway, ask| {
            gateway.apply(&target, &modules, ask)
        })
        .is_some()
    }
}

/// read the plain settings, change them with `edit`, store them with the
/// secret `updates` and apply `modules` (nothing when empty). reading first
/// keeps edits made elsewhere since the window loaded.
pub fn update_settings(
    gateway: &dyn Gateway,
    host: &str,
    updates: &[(String, String)],
    edit: impl FnOnce(&mut Settings) -> Result<()>,
    modules: &[String],
    ask: AskPassword,
) -> Result<()> {
    let mut settings = Settings::parse(&gateway.load_document(host, ask)?.plain)?;
    edit(&mut settings)?;
    gateway.save_document(host, &settings.text(), updates, ask)?;
    if !modules.is_empty() {
        gateway.apply(host, modules, ask)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Fake;

    #[test]
    fn updating_edits_the_stored_text_stores_secrets_and_applies() {
        let fake = Fake::new();
        *fake.stored.lock().unwrap() = "a: 1\nb: 2\n".into();
        let mut ask = |_: &str| Ok(String::new());
        let updates = [("nt4-domain.admin_password".to_string(), "pw".to_string())];
        update_settings(
            &fake,
            "me@gw",
            &updates,
            |s| {
                s.set_section("c", 3.into());
                Ok(())
            },
            &["dns".to_string()],
            &mut ask,
        )
        .unwrap();
        let stored = fake.stored.lock().unwrap().clone();
        assert!(stored.contains("a: 1") && stored.contains("c: 3"));
        assert_eq!(
            fake.calls(),
            ["load me@gw", "save me@gw", "apply me@gw [dns]"]
        );
        assert_eq!(fake.secrets.lock().unwrap().len(), 1);
        // a failing edit stores nothing, and no modules means no apply
        let failed = update_settings(&fake, "me@gw", &[], |_| anyhow::bail!("no"), &[], &mut ask);
        assert!(failed.is_err() && fake.calls().len() == 4);
        update_settings(&fake, "me@gw", &[], |_| Ok(()), &[], &mut ask).unwrap();
        assert_eq!(fake.calls().len(), 6);
    }
}
