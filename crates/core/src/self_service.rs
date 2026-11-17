//! the ops about accounts and permissions: a person's own account, password changes across
//! linked accounts, and the role assignments.

use anyhow::{bail, Result};

use lanlord_model::identity::{AccountInfo, PasswordReport};
use lanlord_model::ops::Reply;
use lanlord_model::secret::Secret;

use crate::access::{self, Access, Caller, Profile};
use crate::bootstrap::AskPassword;
use crate::ssh::Session;
use crate::{admins, config, identity};

/// the longest profile field.
const MAX_FIELD: usize = 128;

/// the gateway account the session signs in as, for the root commands.
fn admin_of(session: &Session) -> Result<String> {
    config::admin(session)
}

fn domain_installed(session: &Session, ask: AskPassword) -> Result<bool> {
    let (settings, _) = access::load(session, ask)?;
    Ok(settings.section("nt4-domain").is_some())
}

pub(crate) fn account_info(session: &Session, caller: &Caller, ask: AskPassword) -> Result<Reply> {
    let access = access::assignments(session, &mut *ask)?;
    let installed = domain_installed(session, &mut *ask)?;
    let admin = admin_of(session)?;
    let links = identity::links_for(session, &admin, installed, ask);
    Ok(Reply::Account(AccountInfo {
        user: caller.user.clone(),
        profile: access
            .profiles
            .get(&caller.user)
            .cloned()
            .unwrap_or_default(),
        roles: access.roles_of(&caller.user, caller.admin),
        grants: access.grants(&caller.user, caller.admin),
        links: identity::infos(&links),
    }))
}

/// set `new` for `user` on every linked account.
fn propagate(
    session: &Session,
    user: &str,
    new: &Secret,
    old: Option<&Secret>,
    ask: AskPassword,
) -> Result<PasswordReport> {
    identity::check_policy(old, new)?;
    let admin = admin_of(session)?;
    let installed = domain_installed(session, &mut *ask)?;
    let links = identity::links_for(session, &admin, installed, ask);
    Ok(identity::change_all(&links, user, new))
}

pub(crate) fn change_password(
    session: &Session,
    caller: &Caller,
    old: &Secret,
    new: &Secret,
    ask: AskPassword,
) -> Result<Reply> {
    identity::check_policy(Some(old), new)?;
    let admin = admin_of(session)?;
    if !identity::verify_password(session, &admin, &caller.user, old, &mut *ask)? {
        bail!("the old password is not correct");
    }
    let report = propagate(session, &caller.user, new, Some(old), ask)?;
    Ok(Reply::Password(report))
}

pub(crate) fn reset_password(
    session: &Session,
    user: &str,
    new: &Secret,
    ask: AskPassword,
) -> Result<Reply> {
    Ok(Reply::Password(propagate(session, user, new, None, ask)?))
}

fn check_field(name: &str, value: &str) -> Result<()> {
    if value.chars().count() > MAX_FIELD || value.chars().any(char::is_control) {
        bail!("{name} is too long or has control characters");
    }
    Ok(())
}

fn save(
    session: &Session,
    host: &str,
    mut settings: lanlord_model::settings::Settings,
    access: &Access,
    ask: AskPassword,
) -> Result<()> {
    access.store(&mut settings);
    config::push_plain_with(session, host, settings.text().as_bytes(), ask)
}

pub(crate) fn update_profile(
    session: &Session,
    host: &str,
    caller: &Caller,
    profile: &Profile,
    ask: AskPassword,
) -> Result<Reply> {
    check_field("the full name", &profile.full_name)?;
    check_field("the description", &profile.description)?;
    let (settings, access) = access::load(session, &mut *ask)?;
    let mut access = access.unwrap_or_default();
    access.profiles.insert(caller.user.clone(), profile.clone());
    save(session, host, settings, &access, ask)?;
    Ok(Reply::Done)
}

pub(crate) fn load_access(session: &Session, ask: AskPassword) -> Result<Reply> {
    let (_, access) = access::load(session, &mut *ask)?;
    let access = match access {
        Some(access) => access,
        // not migrated yet: the admins become Administrators, everyone else stays a User
        None => Access::migrate(&admins::list_with(session, ask)?),
    };
    Ok(Reply::Access(access))
}

pub(crate) fn save_access(
    session: &Session,
    host: &str,
    access: &Access,
    ask: AskPassword,
) -> Result<Reply> {
    if let Some(problem) = access.problem() {
        bail!("{problem}");
    }
    let (settings, _) = access::load(session, &mut *ask)?;
    save(session, host, settings, access, ask)?;
    Ok(Reply::Done)
}
