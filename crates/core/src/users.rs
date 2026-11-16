//! the nt4-domain module's logins, kept in the `users` list of your settings.
//! names, admin flags and removals live in the plain settings; passwords are
//! secrets and live in the encrypted ones. changing a login pulls both, edits
//! them here, pushes them back and applies the module.

use anyhow::{bail, Result};

use crate::apply;
use crate::bootstrap::AskPassword;
use crate::settings::{self, Document};
use crate::ssh::Session;

pub use lanlord_model::users::*;

/// remembers the first password it is given, so one change asks once.
pub fn cached<'a>(ask: AskPassword<'a>) -> impl FnMut(&str) -> Result<String> + 'a {
    let mut saved: Option<String> = None;
    move |what| {
        if saved.is_none() {
            saved = Some(ask(what)?);
        }
        Ok(saved.clone().unwrap_or_default())
    }
}

fn document(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<Document> {
    let document = settings::load_document_with(session, identity, ask)?;
    if document == Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    Ok(document)
}

/// the users your settings define on `host`.
pub fn list_on(
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    let session = Session::new(host)?;
    list(
        document(&session, host, identity, ask_password)?
            .plain
            .as_bytes(),
    )
}

/// make the change in your settings on `host`, apply the module and return the users now defined.
pub fn change(
    host: &str,
    identity: &age::x25519::Identity,
    change: &Change,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    change_all(host, identity, std::slice::from_ref(change), ask_password)
}

/// make several changes in one go (one pull, one push, one apply).
pub fn change_all(
    host: &str,
    identity: &age::x25519::Identity,
    changes: &[Change],
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    change_with(&Session::new(host)?, host, identity, changes, ask_password)
}

fn change_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    changes: &[Change],
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    let mut ask = cached(ask_password);
    let stored = document(session, host, identity, &mut ask)?;
    let edited = edit_split(stored.plain.as_bytes(), changes)?;
    settings::save_changes_with(
        session,
        host,
        identity,
        &edited.plain,
        &edited.secrets,
        &edited.removed,
        &mut ask,
    )?;
    apply::apply_with(
        session,
        host,
        Some(identity),
        &[MODULE.to_string()],
        &mut ask,
    )?;
    list(edited.plain.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = "docker:\n  internal_address: 172.16.120.2/22\nnt4-domain:\n  mode: domain\n  name: RETRONET\n  users:\n    - name: dana\n      password: one\n    - name: gone\n      state: absent\n";

    fn names(settings: &[u8]) -> Vec<String> {
        list(settings)
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect()
    }

    #[test]
    fn list_hides_users_marked_absent() {
        assert_eq!(names(SETTINGS.as_bytes()), ["dana"]);
    }

    #[test]
    fn list_without_a_users_key_is_empty() {
        let settings = b"nt4-domain:\n  mode: workgroup\n  name: WG\n";
        assert!(names(settings).is_empty());
    }

    #[test]
    fn adding_a_user_keeps_everything_else() {
        let change = Change::SetPassword {
            name: "olive".into(),
            password: "pw".into(),
        };
        let out = edit(SETTINGS.as_bytes(), &change).unwrap();
        assert_eq!(names(&out), ["dana", "olive"]);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("internal_address: 172.16.120.2/22"));
        assert!(text.contains("name: RETRONET"));
    }

    #[test]
    fn setting_a_password_replaces_the_old_one_and_revives_absent_users() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::SetPassword {
                name: "dana".into(),
                password: "two".into(),
            },
        )
        .unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("password: two") && !text.contains("password: one"));
        let out = edit(
            &out,
            &Change::SetPassword {
                name: "gone".into(),
                password: "back".into(),
            },
        )
        .unwrap();
        assert_eq!(names(&out), ["dana", "gone"]);
    }

    #[test]
    fn removing_marks_the_user_absent_and_drops_the_password() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::Remove {
                name: "dana".into(),
            },
        )
        .unwrap();
        assert!(names(&out).is_empty());
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("password: one"));
        assert_eq!(text.matches("state: absent").count(), 2);
    }

    #[test]
    fn removing_an_unlisted_user_still_records_the_removal() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::Remove {
                name: "stranger".into(),
            },
        )
        .unwrap();
        assert!(String::from_utf8(out).unwrap().contains("stranger"));
    }

    #[test]
    fn bad_input_is_refused() {
        for name in ["", "Bob", "1abc", "a b", "x;y"] {
            let change = Change::Remove { name: name.into() };
            assert!(edit(SETTINGS.as_bytes(), &change).is_err(), "{name}");
        }
        let empty = Change::SetPassword {
            name: "bob".into(),
            password: String::new(),
        };
        assert!(edit(SETTINGS.as_bytes(), &empty).is_err());
    }

    #[test]
    fn settings_without_the_module_explain_what_to_add() {
        let err = list(b"docker:\n  a: 1\n").unwrap_err().to_string();
        assert!(err.contains("no nt4-domain section"), "{err}");
        assert!(list(b"- just\n- a list\n").is_err());
        assert!(list(b"").is_err(), "empty settings have no module either");
    }

    #[test]
    fn the_password_is_asked_for_once() {
        let mut calls = 0;
        let mut ask = |_: &str| {
            calls += 1;
            Ok("pw".to_string())
        };
        let mut once = cached(&mut ask);
        assert_eq!(once("a").unwrap(), "pw");
        assert_eq!(once("b").unwrap(), "pw");
        drop(once);
        assert_eq!(calls, 1);
    }

    #[test]
    fn changing_a_password_keeps_the_admin_flag() {
        let settings =
            "nt4-domain:\n  users:\n    - name: dana\n      password: one\n      admin: true\n";
        let out = edit(
            settings.as_bytes(),
            &Change::SetPassword {
                name: "dana".into(),
                password: "two".into(),
            },
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("password: two") && text.contains("admin: true"),
            "{text}"
        );
    }

    #[test]
    fn admin_can_be_granted_and_revoked() {
        let granted = edit(
            SETTINGS.as_bytes(),
            &Change::SetAdmin {
                name: "dana".into(),
                admin: true,
            },
        )
        .unwrap();
        assert!(String::from_utf8(granted.clone())
            .unwrap()
            .contains("admin: true"));
        let revoked = edit(
            &granted,
            &Change::SetAdmin {
                name: "dana".into(),
                admin: false,
            },
        )
        .unwrap();
        assert!(!String::from_utf8(revoked).unwrap().contains("admin"));
    }

    #[test]
    fn software_admin_can_be_granted_and_revoked_and_shows_in_the_list() {
        let granted = edit(
            SETTINGS.as_bytes(),
            &Change::SetSoftwareAdmin {
                name: "dana".into(),
                software_admin: true,
            },
        )
        .unwrap();
        assert!(String::from_utf8(granted.clone())
            .unwrap()
            .contains("software_admin: true"));
        let dana = list(&granted).unwrap().remove(0);
        assert!(dana.software_admin && !dana.admin);
        assert_eq!(dana.roles(), ["software admin"]);
        let revoked = edit(
            &granted,
            &Change::SetSoftwareAdmin {
                name: "dana".into(),
                software_admin: false,
            },
        )
        .unwrap();
        assert!(!String::from_utf8(revoked.clone())
            .unwrap()
            .contains("software_admin"));
        assert!(list(&revoked).unwrap()[0].roles().is_empty());
        let both = User {
            name: "x".into(),
            admin: true,
            software_admin: true,
        };
        assert_eq!(both.roles(), ["domain admin", "software admin"]);
    }

    #[test]
    fn software_admin_needs_a_listed_user_and_stays_plain() {
        let err = edit(
            SETTINGS.as_bytes(),
            &Change::SetSoftwareAdmin {
                name: "nobody".into(),
                software_admin: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not in your settings"), "{err}");
        let edited = edit_split(
            SETTINGS.as_bytes(),
            &[Change::SetSoftwareAdmin {
                name: "dana".into(),
                software_admin: true,
            }],
        )
        .unwrap();
        assert!(edited.plain.contains("software_admin: true"));
        assert!(edited.secrets.is_empty() && edited.removed.is_empty());
    }

    #[test]
    fn only_listed_users_can_be_made_admin() {
        let err = edit(
            SETTINGS.as_bytes(),
            &Change::SetAdmin {
                name: "nobody".into(),
                admin: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not in your settings"), "{err}");
    }

    #[test]
    fn a_user_entry_that_is_not_a_mapping_is_reported() {
        let settings = b"nt4-domain:\n  users:\n    - dana\n";
        // a bare name is not a user entry, so it is never matched; a mapping without a name is skipped
        let out = edit(
            settings,
            &Change::SetPassword {
                name: "olive".into(),
                password: "pw".into(),
            },
        )
        .unwrap();
        assert_eq!(names(&out), ["olive"]);
        let scalar = b"nt4-domain:\n  users:\n    - name: dana\n";
        let granted = edit(
            scalar,
            &Change::SetAdmin {
                name: "dana".into(),
                admin: true,
            },
        )
        .unwrap();
        assert!(String::from_utf8(granted).unwrap().contains("admin: true"));
    }

    #[test]
    fn revoking_admin_from_a_plain_user_changes_nothing_else() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::SetAdmin {
                name: "dana".into(),
                admin: false,
            },
        )
        .unwrap();
        assert_eq!(names(&out), ["dana"]);
        assert!(String::from_utf8(out).unwrap().contains("password: one"));
    }

    #[test]
    fn setting_a_password_for_an_absent_user_brings_them_back_with_their_admin_flag() {
        let settings =
            b"nt4-domain:\n  users:\n    - name: gone\n      state: absent\n      admin: true\n";
        let out = edit(
            settings,
            &Change::SetPassword {
                name: "gone".into(),
                password: "back".into(),
            },
        )
        .unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            !text.contains("absent") && text.contains("admin: true"),
            "{text}"
        );
        assert_eq!(names(&out), ["gone"]);
    }

    #[test]
    fn edit_split_keeps_passwords_out_of_the_plain_text() {
        let edited = edit_split(
            SETTINGS.as_bytes(),
            &[
                Change::SetPassword {
                    name: "olive".into(),
                    password: "pw".into(),
                },
                Change::SetAdmin {
                    name: "olive".into(),
                    admin: true,
                },
                Change::Remove {
                    name: "dana".into(),
                },
            ],
        )
        .unwrap();
        assert!(!edited.plain.contains("pw") && !edited.plain.contains("password"));
        assert!(edited.plain.contains("admin: true"));
        assert_eq!(
            edited.secrets,
            [(
                "nt4-domain.users[olive].password".to_string(),
                "pw".to_string()
            )]
        );
        assert_eq!(edited.removed, ["nt4-domain.users[dana].password"]);
        let users = list(edited.plain.as_bytes()).unwrap();
        assert_eq!(
            users,
            [User {
                name: "olive".into(),
                admin: true,
                software_admin: false,
            }]
        );
    }

    #[test]
    fn the_last_change_to_a_user_wins() {
        let set = |name: &str| Change::SetPassword {
            name: name.into(),
            password: "new".into(),
        };
        let remove = |name: &str| Change::Remove { name: name.into() };
        let edited = edit_split(SETTINGS.as_bytes(), &[set("dana"), remove("dana")]).unwrap();
        assert!(edited.secrets.is_empty() && edited.removed.len() == 1);
        let edited = edit_split(SETTINGS.as_bytes(), &[remove("dana"), set("dana")]).unwrap();
        assert!(edited.removed.is_empty() && edited.secrets.len() == 1);
        assert!(edit_split(b"docker: {}\n", &[set("dana")]).is_err());
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::{identity, no_password, Gateway};

        const LEGACY: &str = "docker:\n  internal_address: 172.16.120.2/22\nnt4-domain:\n  mode: domain\n  name: RETRONET\n  users:\n    - name: dana\n      password: one\n      admin: true\n";

        #[test]
        fn a_change_stores_names_plain_and_passwords_encrypted() {
            let (gw, id) = (Gateway::new("users-change"), identity("users-change"));
            gw.seed_sealed(&id, LEGACY);
            let change = Change::SetPassword {
                name: "olive".into(),
                password: "pw".into(),
            };
            let now = change_with(
                &gw.session,
                "gw",
                &id,
                std::slice::from_ref(&change),
                &mut no_password,
            )
            .unwrap();
            let names: Vec<_> = now.iter().map(|u| (u.name.as_str(), u.admin)).collect();
            assert_eq!(names, [("dana", true), ("olive", false)]);

            let plain = gw.plain().unwrap();
            assert!(!plain.contains("one") && !plain.contains("pw"), "{plain}");
            assert!(plain.contains("olive") && plain.contains("admin: true"));
            let sealed = gw.sealed(&id).unwrap();
            assert!(sealed.contains("one") && sealed.contains("pw"), "{sealed}");
            assert!(!sealed.contains("RETRONET"), "{sealed}");
        }

        #[test]
        fn software_admin_is_stored_plain_and_applied() {
            let (gw, id) = (Gateway::new("users-software"), identity("users-software"));
            gw.seed_sealed(&id, LEGACY);
            let change = Change::SetSoftwareAdmin {
                name: "dana".into(),
                software_admin: true,
            };
            let now = change_with(&gw.session, "gw", &id, &[change], &mut no_password).unwrap();
            assert!(now[0].software_admin);
            assert!(gw.plain().unwrap().contains("software_admin: true"));
            assert!(gw.sealed(&id).unwrap().contains("one"), "password kept");
        }

        #[test]
        fn removing_a_user_forgets_the_password_and_admin_changes_touch_only_plain() {
            let (gw, id) = (Gateway::new("users-remove"), identity("users-remove"));
            gw.seed_sealed(&id, LEGACY);
            let admin = Change::SetAdmin {
                name: "dana".into(),
                admin: false,
            };
            change_with(&gw.session, "gw", &id, &[admin], &mut no_password).unwrap();
            assert!(gw.sealed(&id).unwrap().contains("one"), "kept");
            let remove = Change::Remove {
                name: "dana".into(),
            };
            change_with(&gw.session, "gw", &id, &[remove], &mut no_password).unwrap();
            assert!(!gw.sealed(&id).unwrap().contains("one"));
            assert!(gw.plain().unwrap().contains("state: absent"));
        }

        #[test]
        fn the_users_come_from_the_plain_settings() {
            let (gw, id) = (Gateway::new("users-list"), identity("users-list"));
            gw.seed_sealed(&id, LEGACY);
            let doc = document(&gw.session, "gw", &id, &mut no_password).unwrap();
            assert_eq!(names(doc.plain.as_bytes()), ["dana"]);
        }

        #[test]
        fn changing_users_needs_settings_on_the_gateway() {
            let (gw, id) = (Gateway::new("users-none"), identity("users-none"));
            let change = Change::Remove {
                name: "dana".into(),
            };
            let err = change_with(
                &gw.session,
                "gw",
                &id,
                std::slice::from_ref(&change),
                &mut no_password,
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains("no settings on gw"), "{err}");
            assert!(gw.plain().is_none() && gw.sealed(&id).is_none());
        }
    }
}
