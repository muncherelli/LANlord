//! what every window needs to reach a gateway: the gateway itself (real or
//! fake), the task runner and the egui context to wake up.

use std::sync::Arc;

use anyhow::Result;
use eframe::egui::Context;
use lanlord_core::bootstrap::AskPassword;

use crate::gateway::Gateway;
use crate::tasks::{Job, Tasks};

pub struct Services {
    pub gateway: Arc<dyn Gateway>,
    pub tasks: Tasks,
    pub ctx: Context,
}

impl Services {
    pub fn new(gateway: Arc<dyn Gateway>, ctx: &Context) -> Self {
        Self {
            gateway,
            tasks: Tasks::default(),
            ctx: ctx.clone(),
        }
    }

    /// is a task running against `host`?
    pub fn busy(&self, host: &str) -> bool {
        self.tasks.busy(host)
    }

    /// run `work` on a thread with the gateway; `None` when `host` already has a task.
    pub fn spawn<T: Send + 'static>(
        &self,
        host: &str,
        work: impl FnOnce(&dyn Gateway, AskPassword) -> Result<T> + Send + 'static,
    ) -> Option<Job<T>> {
        let gateway = self.gateway.clone();
        self.tasks
            .spawn(&self.ctx, host, move |ask| work(&*gateway, ask))
    }

    /// bootstrap `host` in the background; progress goes to the console.
    pub fn bootstrap(&self, host: &str) -> bool {
        let target = host.to_string();
        self.spawn(host, move |gateway, ask| gateway.bootstrap(&target, ask))
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
