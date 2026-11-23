//! fixture tests for the stored settings: an old file in, the new format out.

use lanlord_model::format::{Context, FORMAT_KEY};
use lanlord_model::settings::Settings;
use lanlord_model::stored::{load, refuse_newer, seal, Kind};

const V1: &str = include_str!("fixtures/stored/plain-v1-legacy.yml");
const V3: &str = include_str!("fixtures/stored/plain-v3-networks.yml");
const V4: &str = include_str!("fixtures/stored/plain-v4-internet.yml");
const NEWER: &str = include_str!("fixtures/stored/plain-v3-newer.yml");

fn admins() -> Context {
    Context {
        admins: Some(vec!["dana".to_string()]),
    }
}

#[test]
fn step_one_turns_the_single_network_into_a_list() {
    let settings = load(Kind::Plain, V1, &Context::default()).unwrap();
    // the administrators are not known here, so the load stops after this step
    assert_eq!(settings.format().unwrap().0, Some(2));
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
    assert_eq!(settings.format().unwrap().0, Some(Kind::Plain.current()));
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
}

#[test]
fn saving_an_old_file_runs_the_steps_that_can_run_and_does_not_claim_the_rest() {
    let saved = seal(Kind::Plain, V1).unwrap();
    let settings = Settings::parse(&saved).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(2), "{saved}");
    assert!(settings.section("networks").is_some(), "{saved}");
    assert!(settings.section("access").is_none(), "{saved}");
    // the upgrade knows the administrators and finishes the job
    let upgraded = load(Kind::Plain, &saved, &admins()).unwrap();
    assert_eq!(upgraded.format().unwrap().0, Some(Kind::Plain.current()));
    assert!(upgraded.section("access").is_some());
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

#[test]
fn step_three_stamps_networks_that_may_allow_internet_and_changes_nothing_else() {
    let settings = load(Kind::Plain, V3, &admins()).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(Kind::Plain.current()));
    assert_eq!(Kind::Plain.current(), 7);
    assert!(settings.text().contains("name: lab"));
    assert!(!settings.text().contains("internet"));
    // a file with grants is current, and an older lanlord (format 3) refuses it
    let with = seal(
        Kind::Plain,
        &format!("{FORMAT_KEY}: 4\nnetworks:\n  - {{name: lab, interface: eth2, address: 192.0.2.1/24, internet: true}}\n"),
    )
    .unwrap();
    assert!(with.contains("internet: true"), "{with}");
    assert!(lanlord_model::format::check(4, None, 3).is_err());
}

#[test]
fn step_four_stamps_networks_that_may_allow_ssh_and_changes_nothing_else() {
    let settings = load(Kind::Plain, V4, &admins()).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(Kind::Plain.current()));
    let text = settings.text();
    // the keys of format 4 are kept as they were, and no ssh key appears
    assert!(
        text.contains("internet: true") && text.contains("dana-pc"),
        "{text}"
    );
    assert!(!text.contains("ssh"), "{text}");
    // a file with ssh keys is format 5; a lanlord that knows format 4 refuses it
    let with = seal(
        Kind::Plain,
        &format!("{FORMAT_KEY}: 5\nnetworks:\n  - {{name: lab, interface: eth2, address: 192.0.2.1/24, ssh: true}}\n"),
    )
    .unwrap();
    assert!(with.contains("ssh: true"), "{with}");
    assert!(lanlord_model::format::check(5, None, 4).is_err());
    assert!(lanlord_model::format::check(4, None, 4).is_ok());
}

const V5: &str = include_str!("fixtures/stored/plain-v5-nt4-domain.yml");

#[test]
fn step_five_splits_the_nt4_domain_into_a_domain_and_a_server_and_is_stable() {
    let settings = load(Kind::Plain, V5, &admins()).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(Kind::Plain.current()));
    let server = settings.section("nt4-servers").unwrap()[0].clone();
    assert_eq!(server["name"], "nt4-domain");
    assert_eq!(server["role"], "pdc");
    assert_eq!(server["domain"], "RETRODOM");
    assert_eq!(server["netbios_name"], "LANLORD");
    assert_eq!(server["network"], "lab");
    assert_eq!(server["software"], true);
    let domains = settings.section("domains").unwrap();
    assert_eq!(domains["RETRODOM"]["roaming_profiles"], true);
    assert_eq!(domains["RETRODOM"]["machines"][1], "ws2");
    // the logins stay where they were; what moved is gone from the old section
    let old = settings.section("nt4-domain").unwrap();
    assert_eq!(old["admin_password"], "example-secret");
    assert_eq!(old["users"][0]["name"], "dana");
    assert!(old.get("mode").is_none());
    // a second load changes nothing
    let again = load(Kind::Plain, &settings.text(), &admins()).unwrap();
    assert_eq!(again.text(), settings.text());
}

const V6_IIS: &str = include_str!("fixtures/stored/plain-v6-iis.yml");
const V6_NO_IIS: &str = include_str!("fixtures/stored/plain-v6-no-iis.yml");

fn hosting_of(settings: &Settings) -> Option<serde_yaml_ng::Value> {
    settings.section("components")?.get("hosting").cloned()
}

#[test]
fn step_six_turns_hosting_on_with_iis_only_when_iis_was_in_use_and_is_stable() {
    let settings = load(Kind::Plain, V6_IIS, &admins()).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(7));
    let hosting = hosting_of(&settings).expect("hosting is on");
    assert_eq!(hosting["enabled"], true);
    assert_eq!(
        hosting["subcomponents"],
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>("[iis]").unwrap()
    );
    // what was there is kept, and the result reads as hosting on beside the nt4 server
    assert_eq!(settings.section("nt4-domain").unwrap()["name"], "RETRODOM");
    let selection = lanlord_model::components::Selection::from_settings(&settings).unwrap();
    assert_eq!(selection.iis_allowed(), Ok(()));
    assert!(selection.is_enabled("nt4-domain"));
    // a second load changes nothing
    let again = load(Kind::Plain, &settings.text(), &admins()).unwrap();
    assert_eq!(again.text(), settings.text());
}

#[test]
fn step_six_leaves_hosting_off_without_iis_in_use() {
    let settings = load(Kind::Plain, V6_NO_IIS, &admins()).unwrap();
    assert_eq!(settings.format().unwrap().0, Some(7));
    assert!(
        settings.section("components").is_none(),
        "{}",
        settings.text()
    );
    let selection = lanlord_model::components::Selection::from_settings(&settings).unwrap();
    assert!(selection.iis_allowed().is_err());
}

#[test]
fn step_six_keeps_other_components_and_what_the_settings_already_say_about_hosting() {
    let with_components =
        "lanlord_format: 6\nnt4-domain: {}\niis: {}\ncomponents:\n  web-proxy: {}\n";
    let settings = load(Kind::Plain, with_components, &admins()).unwrap();
    let components = settings.section("components").unwrap();
    assert!(components.get("web-proxy").is_some());
    assert_eq!(components["hosting"]["enabled"], true);
    // an explicit choice stays as it is, even off
    let off =
        "lanlord_format: 6\nnt4-domain: {}\niis: {}\ncomponents:\n  hosting: {enabled: false}\n";
    let settings = load(Kind::Plain, off, &admins()).unwrap();
    assert_eq!(
        settings.section("components").unwrap()["hosting"]["enabled"],
        false
    );
    // hosting needs the nt4 server: with it off nothing is switched on
    let no_nt4 = "lanlord_format: 6\niis: {}\ncomponents:\n  nt4-domain: {enabled: false}\n";
    assert!(hosting_of(&load(Kind::Plain, no_nt4, &admins()).unwrap()).is_none());
}
