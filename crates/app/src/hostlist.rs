//! the gateways the desktop manages and which one is selected. the selected
//! host is what every action in every window targets.

use std::path::PathBuf;

#[cfg(feature = "native")]
use lanlord_core::hosts;

/// the browser has no hosts file (and only ever manages the gateway it came from), so the list
/// lives in memory there.
#[cfg(not(feature = "native"))]
mod hosts {
    use std::io;
    use std::path::Path;

    pub fn load(_path: &Path) -> Vec<String> {
        Vec::new()
    }

    pub fn add(_path: &Path, _host: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no hosts file here",
        ))
    }

    pub fn remove(_path: &Path, _host: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no hosts file here",
        ))
    }

    pub fn load_last(_path: &Path) -> Option<String> {
        None
    }

    pub fn save_last(_path: &Path, _target: &str) -> io::Result<()> {
        Ok(())
    }
}

/// is `host` something ssh can reach, like user@gateway?
#[cfg(feature = "native")]
fn check(host: &str) -> Result<(), String> {
    lanlord_core::ssh::Session::new(host)
        .map(drop)
        .map_err(|err| err.to_string())
}

#[cfg(not(feature = "native"))]
fn check(host: &str) -> Result<(), String> {
    if host.chars().any(char::is_whitespace) {
        return Err(format!("{host} is not a host name"));
    }
    Ok(())
}

pub struct HostList {
    path: Option<PathBuf>,
    hosts: Vec<String>,
    selected: usize,
    /// the target last signed in to; the this-server target is kept here, not in the list.
    last: Option<String>,
}

impl HostList {
    /// read the list from `path`; without a path it only lives in memory.
    pub fn new(path: Option<PathBuf>) -> Self {
        let hosts = path.as_deref().map(hosts::load).unwrap_or_default();
        let last = path.as_deref().and_then(hosts::load_last);
        Self {
            path,
            hosts,
            selected: 0,
            last,
        }
    }

    /// the target last signed in to, if one was kept.
    pub fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }

    /// keep `target` as the last sign-in, so the next start opens on it.
    pub fn remember(&mut self, target: &str) {
        self.last = Some(target.to_string());
        if let Some(path) = &self.path {
            let _ = hosts::save_last(path, target);
        }
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// the host actions target, if there is one.
    pub fn selected(&self) -> Option<&str> {
        self.hosts.get(self.selected).map(String::as_str)
    }

    pub fn select(&mut self, index: usize) {
        if index < self.hosts.len() {
            self.selected = index;
        }
    }

    /// add `user@host` and select it; the error says what is wrong with the text.
    pub fn add(&mut self, text: &str) -> Result<(), String> {
        let host = text.trim();
        if host.is_empty() {
            return Err("type a host like user@gateway".into());
        }
        check(host)?;
        let saved = match &self.path {
            Some(path) => hosts::add(path, host).map(|()| self.hosts = hosts::load(path)),
            None => Ok(()),
        };
        // a host that could not be saved is still used for this session
        if (self.path.is_none() || saved.is_err()) && !self.hosts.iter().any(|h| h == host) {
            self.hosts.push(host.to_string());
        }
        self.selected = self.hosts.iter().position(|h| h == host).unwrap_or(0);
        saved.map_err(|err| format!("could not save {host}: {err}"))
    }

    /// drop the selected host from the list and the file.
    pub fn remove_selected(&mut self) -> Result<(), String> {
        let Some(host) = self.selected().map(String::from) else {
            return Err("no host is selected".into());
        };
        match &self.path {
            Some(path) => {
                hosts::remove(path, &host)
                    .map_err(|err| format!("could not remove {host}: {err}"))?;
                self.hosts = hosts::load(path);
            }
            None => self.hosts.retain(|h| *h != host),
        }
        self.selected = self.selected.min(self.hosts.len().saturating_sub(1));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("lanlord-ui-hosts-{}-{name}", std::process::id()))
            .join("hosts")
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn adding_saves_selects_and_validates() {
        let path = temp("add");
        let mut list = HostList::new(Some(path.clone()));
        assert!(list.hosts().is_empty() && list.selected().is_none());
        list.add(" me@one ").unwrap();
        list.add("me@two").unwrap();
        list.add("me@one").unwrap();
        assert_eq!(list.hosts(), ["me@one", "me@two"]);
        assert_eq!(list.selected(), Some("me@one"), "re-adding selects it");
        assert_eq!(hosts::load(&path), ["me@one", "me@two"]);
        assert!(list.add("  ").is_err());
        assert!(list.add("-oProxyCommand=x").is_err());
        assert!(list.add("two words").is_err());
        assert_eq!(HostList::new(Some(path.clone())).hosts().len(), 2);
        cleanup(&path);
    }

    #[test]
    fn a_host_that_cannot_be_saved_is_still_used() {
        // a file where the config directory should be makes saving fail
        let path = temp("unsaved");
        std::fs::create_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
        std::fs::write(path.parent().unwrap(), "not a directory").unwrap();
        let mut list = HostList::new(Some(path.clone()));
        let err = list.add("me@one").unwrap_err();
        assert!(err.contains("could not save"), "{err}");
        assert_eq!(list.selected(), Some("me@one"));
        let _ = std::fs::remove_file(path.parent().unwrap());
    }

    #[test]
    fn removing_updates_the_file_and_the_selection() {
        let path = temp("remove");
        let mut list = HostList::new(Some(path.clone()));
        assert!(list.remove_selected().is_err());
        list.add("me@one").unwrap();
        list.add("me@two").unwrap();
        assert_eq!(list.selected_index(), 1);
        list.remove_selected().unwrap();
        assert_eq!(list.hosts(), ["me@one"]);
        assert_eq!(list.selected(), Some("me@one"));
        assert_eq!(hosts::load(&path), ["me@one"]);
        list.remove_selected().unwrap();
        assert!(list.selected().is_none());
        cleanup(&path);
    }

    #[test]
    fn the_last_sign_in_survives_a_restart() {
        let path = temp("last");
        let mut list = HostList::new(Some(path.clone()));
        assert_eq!(list.last(), None);
        list.remember("me@this-server.localhost");
        assert_eq!(list.last(), Some("me@this-server.localhost"));
        assert!(list.hosts().is_empty());
        let again = HostList::new(Some(path.clone()));
        assert_eq!(again.last(), Some("me@this-server.localhost"));
        cleanup(&path);
    }

    #[test]
    fn selection_stays_in_range() {
        let mut list = HostList::new(None);
        list.add("me@one").unwrap();
        list.add("me@two").unwrap();
        list.add("me@two").unwrap();
        assert_eq!(list.hosts().len(), 2);
        list.select(0);
        assert_eq!(list.selected(), Some("me@one"));
        list.select(9);
        assert_eq!(list.selected_index(), 0);
        list.remove_selected().unwrap();
        assert_eq!(list.hosts(), ["me@two"]);
    }
}
