//! a container that is replaced, never edited: a port of `replace-container.yml`. the old one is
//! renamed `<name>-prev` and stopped, the new one is started, and the old one is removed only once
//! the new one is healthy; when it is not, the new one is removed and the old one comes back.

use std::time::Duration;

use anyhow::{bail, Result};
use lanlord_model::engine::{Action, ChangeView};

use super::{Change, Observed, Resource};
use crate::engine::host::Host;
use crate::engine::journal::{Journal, Undo};
use crate::engine::Events;

const STATE_FORMAT: &str = "{{.State.Running}}";
const STEADY_FORMAT: &str = "{{.State.Running}} {{.State.Restarting}} {{.RestartCount}}";

pub struct Container {
    pub name: String,
    /// what `docker inspect` of the right container says: "true <image id> <scripts label>".
    pub want: String,
    /// the format that produces `want`.
    pub inspect_format: String,
    /// the whole `docker run -d ...` command.
    pub run: Vec<String>,
    /// a command that exits 0 when the service is healthy.
    pub health: Vec<String>,
    pub retries: u32,
    pub delay: Duration,
    pub checks: u32,
    /// ids of resources whose change needs a new container (the configuration it mounts).
    pub follows: Vec<String>,
}

impl Container {
    pub fn new(
        name: &str,
        want: String,
        inspect_format: &str,
        run: Vec<String>,
        health: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            want,
            inspect_format: inspect_format.into(),
            run,
            health,
            retries: 10,
            delay: Duration::from_secs(2),
            checks: 3,
            follows: Vec::new(),
        }
    }

    fn prev(&self) -> String {
        format!("{}-prev", self.name)
    }

    fn docker(&self, host: &dyn Host, args: &[&str]) -> Result<crate::engine::host::Output> {
        let mut argv = vec!["docker"];
        argv.extend_from_slice(args);
        host.run(&argv, None)
    }

    fn must(&self, host: &dyn Host, args: &[&str]) -> Result<()> {
        let out = self.docker(host, args)?;
        if !out.success() {
            bail!(
                "docker {} failed: {}",
                args.first().copied().unwrap_or(""),
                out.stderr.trim()
            );
        }
        Ok(())
    }

    /// running, not restarting and healthy for several checks in a row.
    fn steady(&self, host: &dyn Host) -> Result<(), String> {
        let mut first: Option<String> = None;
        for _ in 0..self.checks {
            let out = self
                .docker(host, &["inspect", "--format", STEADY_FORMAT, &self.name])
                .map_err(|e| e.to_string())?;
            if !out.success() {
                return Err(format!("docker inspect failed: {}", out.stderr.trim()));
            }
            let state = out.stdout.trim().to_string();
            let words: Vec<&str> = state.split_whitespace().collect();
            if words.len() < 3 || words[0] != "true" || words[1] != "false" {
                return Err(format!("not running steadily: {state}"));
            }
            let count = first.get_or_insert_with(|| words[2].to_string());
            if words[2] != count {
                return Err(format!("restarted: {state}"));
            }
            let argv: Vec<&str> = self.health.iter().map(String::as_str).collect();
            let ok = host.run(&argv, None).map(|o| o.success()).unwrap_or(false);
            if !ok {
                return Err(format!("health check failed: {}", self.health.join(" ")));
            }
            host.sleep(self.delay);
        }
        Ok(())
    }

    fn wait_healthy(&self, host: &dyn Host) -> Result<(), String> {
        let mut last = String::new();
        for attempt in 0..self.retries.max(1) {
            match self.steady(host) {
                Ok(()) => return Ok(()),
                Err(why) => last = why,
            }
            if attempt + 1 < self.retries {
                host.sleep(self.delay);
            }
        }
        Err(last)
    }
}

impl Resource for Container {
    fn id(&self) -> String {
        format!("container {}", self.name)
    }

    fn check(&self, host: &dyn Host) -> Result<Observed> {
        let main = self.docker(
            host,
            &["inspect", "--format", &self.inspect_format, &self.name],
        )?;
        let mut observed = Observed {
            exists: main.success(),
            ..Observed::default()
        };
        observed
            .facts
            .insert("current".into(), main.stdout.trim().to_string());
        Ok(observed)
    }

    fn diff(&self, observed: &Observed, changed: &[String]) -> Option<Change> {
        let current = observed
            .facts
            .get("current")
            .map(String::as_str)
            .unwrap_or("");
        let config = self.follows.iter().any(|id| changed.contains(id));
        if current == self.want && !config {
            return None;
        }
        let (action, summary) = if !observed.exists {
            (Action::Create, format!("{} will be started", self.name))
        } else if config && current == self.want {
            (
                Action::Replace,
                format!(
                    "{} will be replaced to read its new configuration",
                    self.name
                ),
            )
        } else {
            (
                Action::Replace,
                format!(
                    "{} will be replaced: it is stopped or out of date",
                    self.name
                ),
            )
        };
        Some(Change {
            view: ChangeView {
                resource: self.id(),
                action,
                summary,
                diff: None,
                sensitive: false,
            },
            observed: observed.clone(),
        })
    }

    fn apply(
        &self,
        host: &dyn Host,
        _: &Change,
        journal: &mut Journal,
        _: &mut Events,
    ) -> Result<Undo> {
        let name = self.name.as_str();
        let prev = self.prev();
        let state = |n: &str| -> Result<(bool, bool)> {
            let out = self.docker(host, &["inspect", "--format", STATE_FORMAT, n])?;
            Ok((out.success(), out.stdout.trim() == "true"))
        };
        let (mut had_old, main_up) = state(name)?;
        let (has_prev, _) = state(&prev)?;

        if has_prev && main_up {
            // a leftover from an interrupted run, the main one is running
            self.must(host, &["rm", "-f", &prev])?;
        } else if has_prev {
            // the interrupted run left the old one aside and the main one is not running
            let _ = self.docker(host, &["rm", "-f", name]);
            self.must(host, &["rename", &prev, name])?;
            self.must(host, &["start", name])?;
            had_old = true;
        }

        let mut swapped = false;
        let tried = (|| -> Result<()> {
            if had_old {
                self.must(host, &["rename", name, &prev])?;
                swapped = true;
                self.must(host, &["stop", &prev])?;
            }
            let run: Vec<&str> = self.run.iter().map(String::as_str).collect();
            let out = host.run(&run, None)?;
            if !out.success() {
                bail!("docker run failed: {}", out.stderr.trim());
            }
            self.wait_healthy(host).map_err(|why| anyhow::anyhow!(why))
        })();

        match tried {
            Ok(()) => {
                let _ = self.docker(host, &["rm", "-f", &prev]);
                Ok(Undo::Irreversible(format!(
                    "{name} was replaced and the previous container removed"
                )))
            }
            Err(error) => {
                let mut logs = String::new();
                if swapped || !had_old {
                    if let Ok(out) = self.docker(host, &["logs", "--tail", "40", name]) {
                        logs = format!("{}{}", out.stdout, out.stderr);
                    }
                    let _ = self.docker(host, &["rm", "-f", name]);
                }
                if swapped {
                    let _ = self.docker(host, &["rename", &prev, name]);
                }
                if had_old {
                    // started by the rollback, once the files it reads are back
                    journal.restart_after_rollback(name);
                }
                let outcome = if had_old {
                    "the previous container was restored and is started again once its files are back."
                } else {
                    "there was no previous container to restore."
                };
                let shown = if logs.trim().is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nlast log lines of the failed {name}:\n{}",
                        logs.trim_end()
                    )
                };
                bail!("replacing {name} failed: {error} -- {outcome}{shown}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{FakeHost, Output};

    fn container() -> Container {
        let mut c = Container::new(
            "lanlord-nt4",
            "true sha256:new abc".into(),
            "{{.State.Running}} {{.Image}}",
            vec!["docker".into(), "run".into(), "-d".into(), "img".into()],
            vec![
                "docker".into(),
                "exec".into(),
                "lanlord-nt4".into(),
                "ping".into(),
            ],
        );
        c.delay = Duration::from_millis(1);
        c.retries = 2;
        c
    }

    fn go(host: &FakeHost, c: &Container) -> Result<Undo> {
        let seen = c.check(host)?;
        let change = c.diff(&seen, &[]).unwrap();
        c.apply(host, &change, &mut Journal::new("/j".into()), &mut |_| {})
    }

    fn up(host: &FakeHost) {
        host.when(
            "docker inspect --format {{.State.Running}} lanlord-nt4-prev",
            vec![Output::failed(1, "no such")],
        );
        host.when(
            "docker inspect --format {{.State.Running}} lanlord-nt4",
            vec![Output::ok("true\n")],
        );
        host.when(
            "docker inspect --format {{.State.Running}} {{.State.Restarting}} {{.RestartCount}}",
            vec![Output::ok("true false 0\n")],
        );
    }

    #[test]
    fn up_to_date_is_no_change_and_follows_a_changed_file() {
        let host = FakeHost::new();
        host.when(
            "docker inspect --format {{.State.Running}} {{.Image}}",
            vec![Output::ok("true sha256:new abc\n")],
        );
        let mut c = container();
        let seen = c.check(&host).unwrap();
        assert!(c.diff(&seen, &[]).is_none());
        c.follows = vec!["file /x".into()];
        let change = c.diff(&seen, &["file /x".into()]).unwrap();
        assert_eq!(change.view.action, Action::Replace);
    }

    #[test]
    fn a_healthy_replacement_removes_the_previous_container() {
        let host = FakeHost::new();
        up(&host);
        let undo = go(&host, &container()).unwrap();
        assert!(matches!(undo, Undo::Irreversible(_)));
        let log = host.log();
        let pos = |p: &str| log.iter().position(|l| l.starts_with(p)).unwrap();
        assert!(
            pos("docker rename lanlord-nt4 lanlord-nt4-prev") < pos("docker stop lanlord-nt4-prev")
        );
        assert!(pos("docker stop lanlord-nt4-prev") < pos("docker run -d"));
        assert!(pos("docker run -d") < pos("docker rm -f lanlord-nt4-prev"));
        assert_eq!(host.ran("docker exec lanlord-nt4 ping"), 3);
    }

    #[test]
    fn an_unhealthy_replacement_is_removed_and_the_old_one_comes_back() {
        let host = FakeHost::new();
        up(&host);
        host.when("docker exec lanlord-nt4 ping", vec![Output::failed(1, "")]);
        host.when("docker logs", vec![Output::ok("boom\n")]);
        let c = container();
        let seen = c.check(&host).unwrap();
        let change = c.diff(&seen, &[]).unwrap();
        let mut journal = Journal::new("/j".into());
        let error = c
            .apply(&host, &change, &mut journal, &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(error.contains("health check failed"), "{error}");
        assert!(error.contains("previous container was restored"), "{error}");
        assert!(error.contains("boom"), "{error}");
        // the old container is not started until the rollback has put its files back
        assert_eq!(host.ran("docker start lanlord-nt4"), 0);
        let report = journal.rollback(&host);
        assert_eq!(report.undone, vec!["started lanlord-nt4 again"]);
        let log = host.log();
        let pos = |p: &str| log.iter().rposition(|l| l.starts_with(p)).unwrap();
        assert!(
            pos("docker rm -f lanlord-nt4") < pos("docker rename lanlord-nt4-prev lanlord-nt4")
        );
        assert!(
            pos("docker rename lanlord-nt4-prev lanlord-nt4") < pos("docker start lanlord-nt4")
        );
        assert_eq!(host.ran("docker rm -f lanlord-nt4-prev"), 0);
    }

    #[test]
    fn a_leftover_previous_container_is_restored_when_the_main_one_is_down() {
        let host = FakeHost::new();
        host.when(
            "docker inspect --format {{.State.Running}} lanlord-nt4-prev",
            vec![Output::ok("false\n")],
        );
        host.when(
            "docker inspect --format {{.State.Running}} lanlord-nt4",
            vec![Output::failed(1, "gone")],
        );
        host.when(
            "docker inspect --format {{.State.Running}} {{.State.Restarting}} {{.RestartCount}}",
            vec![Output::ok("true false 0\n")],
        );
        go(&host, &container()).unwrap();
        let log = host.log();
        let restore = log
            .iter()
            .position(|l| l == "docker rename lanlord-nt4-prev lanlord-nt4")
            .unwrap();
        let aside = log
            .iter()
            .position(|l| l == "docker rename lanlord-nt4 lanlord-nt4-prev")
            .unwrap();
        assert!(restore < aside);
    }

    #[test]
    fn a_restarting_container_is_not_healthy() {
        let host = FakeHost::new();
        up(&host);
        host.when(
            "docker inspect --format {{.State.Running}} {{.State.Restarting}} {{.RestartCount}}",
            vec![Output::ok("true false 0\n"), Output::ok("true false 1\n")],
        );
        let mut c = container();
        c.retries = 1;
        assert!(c.wait_healthy(&host).is_err());
    }
}
