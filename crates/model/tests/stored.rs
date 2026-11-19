//! fixture tests for the stored settings: an old file in, the new format out.

use lanlord_model::format::{Context, FORMAT_KEY};
use lanlord_model::settings::Settings;
use lanlord_model::stored::{load, refuse_newer, seal, Kind};

const V1: &str = include_str!("fixtures/stored/plain-v1-legacy.yml");
const NEWER: &str = include_str!("fixtures/stored/plain-v3-newer.yml");

fn admins() -> Context {
    Context {
        admins: Some(vec!["dana".to_string()]),
    }
}

#[test]
fn step_one_turns_the_single_network_into_a_list() {
    let settings = load(Kind::Plain, V1, &Context::default()).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(Kind::Plain.current()));
    assert!(
        settings.section("networks").is_some(),
        "{}",
        settings.text()
    );
    assert!(settings.section("access").is_none(), "no admins known yet");
}

#[test]
fn step_two_stores_the_role_assignments_once_the_admins_are_known() {
    let settings = load(Kind::Plain, V1, &admins()).unwrap();
    assert!(settings.section("access").is_some(), "{}", settings.text());
    let again = load(Kind::Plain, &settings.text(), &admins()).unwrap();
    assert_eq!(
        again.text(),
        settings.text(),
        "a current file is left alone"
    );
}

#[test]
fn a_current_file_loads_and_saves_unchanged() {
    let text = seal(
        Kind::Plain,
        &load(Kind::Plain, V1, &admins()).unwrap().text(),
    )
    .unwrap();
    let loaded = load(Kind::Plain, &text, &admins()).unwrap();
    assert_eq!(loaded.text(), text);
    assert_eq!(seal(Kind::Plain, &loaded.text()).unwrap(), text);
}

#[test]
fn a_file_with_no_version_is_version_one() {
    assert_eq!(Settings::parse(V1).unwrap().format().unwrap().0, None);
    assert!(!seal(Kind::Plain, V1).unwrap().is_empty());
}

#[test]
fn a_newer_file_is_refused_for_both_kinds_and_never_stamped_over() {
    for kind in [Kind::Plain, Kind::Sealed] {
        let error = load(kind, NEWER, &admins()).unwrap_err().to_string();
        assert_eq!(
            error,
            "written by LANlord 9.9 (format 99); upgrade LANlord to use it"
        );
        assert!(seal(kind, NEWER).is_err());
        assert!(refuse_newer(kind, NEWER).is_err());
    }
    assert!(NEWER.contains(FORMAT_KEY));
}
