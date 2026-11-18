//! developer tools every front end offers in a development build: a red bug that opens a debug
//! menu with a demo desktop on canned data, window-manager outlines and "open all windows".
//! this is what the menu says and which windows "open all" means; each front end draws it and
//! does the rest. compiled only with the `fake` feature, never into a release build.

use crate::desktop::Target;

/// the menu entries, as the gui and the tui word them.
pub const DEMO_DESKTOP: &str = "Demo desktop";
pub const WINDOW_DEBUG: &str = "Window manager debug";
pub const OPEN_ALL: &str = "Open all windows";

/// what the debug menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// switch to the demo desktop and back.
    Demo,
    /// outline every window's contents and padding.
    WindowDebug,
    /// open every window there is.
    OpenAll,
}

/// one row of the debug menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: &'static str,
    pub ticked: bool,
    pub choice: Choice,
}

/// the rows of the debug menu, given whether the demo desktop and the outlines are on.
pub fn entries(demo: bool, window_debug: bool) -> [Entry; 3] {
    [
        Entry {
            label: DEMO_DESKTOP,
            ticked: demo,
            choice: Choice::Demo,
        },
        Entry {
            label: WINDOW_DEBUG,
            ticked: window_debug,
            choice: Choice::WindowDebug,
        },
        Entry {
            label: OPEN_ALL,
            ticked: false,
            choice: Choice::OpenAll,
        },
    ]
}

/// every window "open all windows" opens: the desktop icons and the windows with no icon.
pub fn open_all(icons: &[Target]) -> Vec<Target> {
    let mut targets = icons.to_vec();
    targets.extend([
        Target::DateTime,
        Target::Components,
        Target::Welcome,
        Target::Changelog,
    ]);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_ticks_what_is_on_and_open_all_adds_the_windows_without_icons() {
        let rows = entries(true, false);
        assert_eq!(rows.map(|e| e.ticked), [true, false, false]);
        assert_eq!(
            rows.map(|e| e.choice),
            [Choice::Demo, Choice::WindowDebug, Choice::OpenAll]
        );
        let all = open_all(&[Target::MyServer]);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0], Target::MyServer);
        assert!(all.contains(&Target::Changelog));
    }
}
