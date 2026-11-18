//! what a person's own account looks like to them: the accounts it is linked to and how a
//! password change went on each of them.

use serde::{Deserialize, Serialize};

use crate::access::{Grants, Profile};

/// how one linked account fared in a password change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum LinkState {
    /// the new password is set there.
    Changed,
    /// nothing was done there, and why (the link cannot do it yet).
    Skipped(String),
    /// it failed here, which stopped the change.
    Failed(String),
    /// it was changed, then put back because a later link failed.
    RolledBack,
    /// it was changed and could not be put back; it needs attention.
    RollbackFailed(String),
    /// an earlier link failed, so this one was left alone.
    NotTried,
}

/// one linked account and what happened to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkReport {
    /// what the link is, such as "Linux account".
    pub link: String,
    pub state: LinkState,
}

/// the result of a password change across every link: it counts only when no link failed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordReport {
    pub ok: bool,
    pub links: Vec<LinkReport>,
}

/// the signed-in person's own account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub user: String,
    pub profile: Profile,
    /// the role names the person has.
    pub roles: Vec<String>,
    /// the level of every area for the person, with the roles worked in.
    pub grants: Grants,
    /// the linked accounts and whether each is in use.
    pub links: Vec<LinkInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    pub link: String,
    /// a short status such as "active" or "not available yet".
    pub status: String,
}
