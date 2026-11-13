//! What a user who is not the administrator may create, alter and drop in a
//! dictionary, as the original engine refuses it (`fixtures/sql/ddfuncs`,
//! `ddrights`): with 5054 and what the user is not permitted.

use tflip_storage::dd::Kind;

use super::permissions;
use crate::ace_error;
use crate::handle::Handle;
use crate::{Error, Session, connection};

/// The right to alter an object.
pub(crate) const ALTER: u32 = 0x100;
/// The right to drop an object.
pub(crate) const DROP: u32 = 0x200;

/// 5054, what the connection's user is not permitted: `detail`.
pub(crate) fn refuse(detail: &str) -> Error {
    ace_error::sql(5054, detail)
}

/// Whether the connection's user may create objects of the type `object_type`.
pub(crate) fn may_create(conn: &connection::Connection, object_type: u16) -> bool {
    conn.with_database_opt(|db| db.may_create_type(object_type))
        .unwrap_or(true)
}

/// 5054, "Cannot create <what> object in the data dictionary.", unless the
/// connection's user may create objects of `object_type`: what the calls that
/// add an object refuse a user with (`fixtures/sql/ddbuiltin`).
pub(crate) fn require_create(
    conn: &connection::Connection,
    object_type: u16,
    what: &str,
) -> Result<(), Error> {
    match may_create(conn, object_type) {
        true => Ok(()),
        false => Err(refuse(&format!(
            "Cannot create {what} object in the data dictionary."
        ))),
    }
}

/// Whether the connection's user holds the right `bit` on the object of
/// `object_type` named `name`.
pub(crate) fn may(conn: &connection::Connection, object_type: u16, name: &str, bit: u32) -> bool {
    conn.with_database_opt(|db| db.may(object_type, name, bit))
        .unwrap_or(true)
}

/// 5054, with `detail` (none for the bare refusal), unless the connection's
/// user holds the right `bit` on the object of `object_type` named `name`: what
/// the `AdsDD` calls that change or drop an object refuse a user with
/// (`fixtures/sql/ddapirights`).
pub(crate) fn require(
    conn: &connection::Connection,
    object_type: u16,
    name: &str,
    bit: u32,
    detail: &str,
) -> Result<(), Error> {
    match may(conn, object_type, name, bit) {
        true => Ok(()),
        false => Err(refuse(detail)),
    }
}

/// 5054 unless the connection's user is the administrator or a member of
/// `DB:Admin`, as `AdsDDSetDatabaseProperty` holds it to.
pub(crate) fn require_administrator(conn: &connection::Connection) -> Result<(), Error> {
    let held =
        conn.with_database_opt(|db| db.user().is_none() || db.is_admin() || db.is_admin_member());
    match held.unwrap_or(true) {
        true => Ok(()),
        false => Err(refuse("")),
    }
}

/// Gives a user who made the object `name` (of `object_type`) the rights on
/// it: all of them, with the grant option.
pub(crate) fn grant_creator(
    session: &Session,
    connection: Handle,
    object_type: u16,
    name: &str,
) -> Result<(), Error> {
    let conn = connection::get(session, connection)?;
    let Some(user) = conn.with_database_opt(|db| db.creator()).flatten() else {
        return Ok(());
    };
    permissions::grant_to_creator(
        session,
        connection,
        object_type,
        name,
        &user,
        permissions::ALL_WITH_GRANT,
    )
}

/// 5054 with `detail` unless the connection's user holds the right `bit` on
/// the table `name`, when the dictionary has it: a free table is nobody's
/// to refuse.
pub(crate) fn require_table(
    session: &Session,
    connection: Handle,
    name: &str,
    bit: u32,
    detail: &str,
) -> Result<(), Error> {
    let conn = connection::get(session, connection)?;
    let refused = conn.with_database_opt(|db| {
        db.dictionary().named(Kind::Table, name).is_some() && !db.may(1, name, bit)
    });
    match refused {
        Some(true) => Err(refuse(detail)),
        _ => Ok(()),
    }
}
