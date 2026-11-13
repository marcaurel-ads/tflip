//! Granting and revoking permissions in a data dictionary
//! (`AdsDDGrantPermission`/`AdsDDRevokePermission`, and through them SQL `GRANT`
//! and `REVOKE`), as the original engine does — probed black-box with
//! `tools/ace/dd/pubperm.c` (`fixtures/dictionary/pubperm.json`) and
//! `tools/ace/dd/grants.c` (`fixtures/dictionary/grants.json`).
//!
//! The rights on an object live in the grantee's Permission object for it,
//! enciphered: slot 1 the object rights, slot 3 those held with the grant option
//! and, on a table, slots 2 and 4 the same per column (one byte each, in field
//! order). The rights to create objects live in the grantee's own User or Group
//! object (slot 6, one byte per kind).

use std::path::Path;

use tflip_storage::crypto;
use tflip_storage::dd::{self, Dictionary, Kind, Object, create::NewObject, permission};

use crate::ace_error;

use super::{
    COLUMN_RIGHTS_MASK, dictionary_connection, open_dict, raise_permission_version, read_next_id,
    rebuild_dictionary, write_object_slots,
};
use crate::handle::Handle;
use crate::{Error, Session, connection};

/// `ADS_PERMISSION_ALL`: every right the object's type allows (and, to a user,
/// the inherit bit), without the grant option.
const ALL: u32 = 0xFFFF_FFFF;
/// `ADS_PERMISSION_ALL_WITH_GRANT`: `ALL` with the grant option.
pub(super) const ALL_WITH_GRANT: u32 = 0x8FFF_FFFF;
/// The `ADS_PERMISSION_*` bits there are; any other (but the grant option) is
/// refused.
const DEFINED: u32 = 0x3FF;
const EXECUTE: u32 = 0x0004;
const LINK_ACCESS: u32 = 0x0040;
const CREATE: u32 = 0x0080;
const ALTER: u32 = 0x0100;
const DROP: u32 = 0x0200;

/// 5132 for a permission with no object name (`AdsDDGrantPermission` with a null
/// name, or a SQL permission on an object without `ON`), which the engine refuses
/// before touching the dictionary.
pub(super) fn empty_object_name() -> Error {
    ace_error::sql(5132, "The object name cannot be empty.")
}

/// What the engine knows of an object type that has permissions.
struct TypeInfo {
    object_type: u16,
    kind: Option<Kind>,
    /// The rights a grant on it may carry (besides the inherit bit).
    valid: u32,
    /// How "There is no such named … in the data dictionary." names it.
    named: &'static str,
    /// The engine's complaint about a right the type does not allow.
    invalid: &'static str,
}

/// The object types that have permissions.
const TYPES: [TypeInfo; 12] = [
    TypeInfo {
        object_type: 1,
        kind: Some(Kind::Table),
        valid: permission::TABLE_RIGHTS,
        named: "table",
        invalid: "One of the specified permissions is not a valid table permission.",
    },
    TypeInfo {
        object_type: 4,
        kind: Some(Kind::Field),
        valid: COLUMN_RIGHTS_MASK,
        named: "column",
        invalid: "One of the specified permissions is not a valid column permission.",
    },
    TypeInfo {
        object_type: 6,
        kind: Some(Kind::View),
        valid: permission::TABLE_RIGHTS,
        named: "view",
        invalid: "One of the specified permissions is not a valid view permission.",
    },
    TypeInfo {
        object_type: 10,
        kind: Some(Kind::Procedure),
        valid: EXECUTE | ALTER | DROP,
        named: "stored procedure",
        invalid: "One of the specified permissions is not a valid stored procedures permission.",
    },
    TypeInfo {
        object_type: 18,
        kind: Some(Kind::Function),
        valid: EXECUTE | ALTER | DROP,
        named: "function",
        invalid: "One of the specified permission is not a valid function permission.",
    },
    TypeInfo {
        object_type: 19,
        kind: Some(Kind::Package),
        valid: EXECUTE | ALTER | DROP,
        named: "package",
        invalid: "One of the specified permission is not a valid package permission.",
    },
    TypeInfo {
        object_type: 12,
        kind: Some(Kind::Link),
        valid: LINK_ACCESS | ALTER | DROP,
        named: "link",
        invalid: "One of the specified permissions is not a valid link permission.",
    },
    TypeInfo {
        object_type: 15,
        kind: Some(Kind::Publication),
        valid: ALTER | DROP,
        named: "publication",
        invalid: "One of the specified permissions is not a valid publication permission.",
    },
    TypeInfo {
        object_type: 17,
        kind: Some(Kind::Subscription),
        valid: ALTER | DROP,
        named: "subscription",
        invalid: "One of the specified permissions is not a valid subscription permission.",
    },
    TypeInfo {
        object_type: 8,
        kind: Some(Kind::User),
        valid: ALTER | DROP,
        named: "user",
        invalid: "One of the specified permissions is not a valid user permission.",
    },
    TypeInfo {
        object_type: 9,
        kind: Some(Kind::Group),
        valid: ALTER | DROP,
        named: "user group",
        invalid: "One of the specified permissions is not a valid user group permission.",
    },
    TypeInfo {
        object_type: 11,
        kind: None,
        valid: ALTER,
        named: "database",
        invalid: "One of the specified permissions is not a valid database permission.",
    },
];

/// 5158 for a right the kind of object `kind` does not allow.
pub(super) fn invalid_right(kind: Kind) -> Error {
    let info = TYPES.iter().find(|t| t.kind == Some(kind));
    ace_error::sql(5158, info.map_or("", |t| t.invalid))
}

fn type_info(object_type: u16) -> Option<&'static TypeInfo> {
    TYPES.iter().find(|t| t.object_type == object_type)
}

/// The kinds object type 0 (`ADS_DD_UNKNOWN_OBJECT`) finds a name among, in the
/// order tflip looks: the original refuses a name already in use by any of them,
/// so at most one matches.
const ANY_KIND: [u16; 10] = [1, 6, 10, 18, 19, 12, 15, 17, 8, 9];

/// The rights `info`'s type allows, the inherit bit included (not on a column).
fn allowed(info: &TypeInfo) -> u32 {
    if info.object_type == 4 {
        info.valid
    } else {
        info.valid | permission::INHERIT
    }
}

/// Who a permission is granted to.
enum Grantee<'a> {
    User(&'a Object),
    Group(&'a Object),
}

impl Grantee<'_> {
    fn object(&self) -> &Object {
        match self {
            Grantee::User(o) | Grantee::Group(o) => o,
        }
    }
}

/// Resolves `name` to a user or a group. `Ok(None)` for `DB:Public`, whose
/// rights the engine keeps in the object itself; `on_built_in` for another
/// built-in group; 5132 for no such grantee.
fn resolve_grantee<'a>(
    dict: &'a Dictionary,
    name: &str,
    on_built_in: impl Fn(&str) -> Error,
) -> Result<Option<Grantee<'a>>, Error> {
    if let Some(user) = dict.named(Kind::User, name) {
        return Ok(Some(Grantee::User(user)));
    }
    if let Some(group) = dict.named(Kind::Group, name) {
        return Ok(Some(Grantee::Group(group)));
    }
    match dd::built_in_group(name) {
        Some("DB:Public") => Ok(None),
        Some(built_in) => Err(on_built_in(built_in)),
        None => Err(ace_error::sql(
            5132,
            "The user or user group is not found in the database.",
        )),
    }
}

/// The object a permission is about.
struct Target {
    info: &'static TypeInfo,
    /// The object holding the Permission object: the object itself, a column's
    /// table.
    id: u32,
    /// For a table (and so a column), its field count.
    fields: Option<usize>,
    /// For a column, its position in field order.
    column: Option<usize>,
}

/// A table's field count.
fn field_count(dict: &Dictionary, table: &Object) -> usize {
    dict.children(table.id, Kind::Field).len()
}

/// Resolves the object of a permission change of `object_type`.
fn resolve_target(
    dict: &Dictionary,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
) -> Result<Target, Error> {
    let target = |info: &'static TypeInfo, object: &Object| Target {
        info,
        id: object.id,
        fields: (info.object_type == 1).then(|| field_count(dict, object)),
        column: None,
    };
    let built_in = || dd::built_in_group(object_name).is_some();
    match object_type {
        0 => {
            for object_type in ANY_KIND {
                let info = type_info(object_type).unwrap();
                if let Some(object) = dict.named_in_database(info.kind.unwrap(), object_name) {
                    return Ok(target(info, object));
                }
            }
            if !object_name.is_empty() && built_in() {
                return Err(ace_error::sql(
                    5158,
                    "The specified permission is not valid for the database roles. Database \
                     roles cannot be modified or dropped.",
                ));
            }
            Err(ace_error::sql(5137, "The given object was not found."))
        }
        4 => {
            let info = type_info(4).unwrap();
            let table = dict.named(Kind::Table, parent_name).ok_or_else(|| {
                ace_error::sql(5132, "There is no such named table in the data dictionary.")
            })?;
            let fields = dict.children(table.id, Kind::Field);
            let column = fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(object_name))
                .ok_or_else(|| {
                    ace_error::sql(5132, "The specified column is not found in the table. ")
                })?;
            Ok(Target {
                info,
                id: table.id,
                fields: Some(fields.len()),
                column: Some(column),
            })
        }
        11 => {
            let database = dict.database().ok_or(Error::NoDictionary)?;
            Ok(target(type_info(11).unwrap(), database))
        }
        _ => {
            let info = type_info(object_type).unwrap();
            if let Some(object) = dict.named_in_database(info.kind.unwrap(), object_name) {
                return Ok(target(info, object));
            }
            Err(match object_type {
                9 if built_in() => {
                    ace_error::sql(5158, "Privilege to system roles is not grantable.")
                }
                18 => ace_error::sql(
                    5158,
                    "Cannot grant or revoke permissions to functions in a package.",
                ),
                _ => ace_error::sql(
                    5132,
                    &format!(
                        "There is no such named {} in the data dictionary.",
                        info.named
                    ),
                ),
            })
        }
    }
}

/// The rights a user who is not the administrator holds with the grant option
/// on `target`: it gives and takes away only those, and `ALL` is never refused
/// but cut to them. `None` where the connection is held to nothing (`checked`
/// off, or the administrator); 5054 for rights beyond them. With `whole_of`,
/// the columns of the table, those it holds on all of them.
fn grantable_rights(
    conn: &connection::Connection,
    target: &Target,
    (object_name, parent_name): (&str, &str),
    (all, base): (bool, u32),
    (checked, whole_of): (bool, Option<&[String]>),
) -> Result<Option<u32>, Error> {
    let grantable = conn
        .with_database_opt(|db| {
            let (name, parent) = match target.column {
                Some(_) => (object_name, parent_name),
                None => (object_name, ""),
            };
            db.grantable(target.info.object_type, name, parent)
        })
        .flatten()
        .filter(|_| checked);
    // Only what it holds on every column, for a grantee that is given rights
    // on the table alone.
    let grantable = match (grantable, whole_of) {
        (Some(held), Some(columns)) => conn
            .with_database_opt(|db| {
                (columns.iter()).fold(held, |mask, name| {
                    mask & db.grantable(4, name, object_name).unwrap_or(mask)
                })
            })
            .or(Some(held)),
        (held, _) => held,
    };
    if let Some(held) = grantable
        && !all
        && base & !permission::INHERIT & !held != 0
    {
        let named = match (target.info.object_type, target.column) {
            (1, _) => format!("the table [{}]", object_name),
            (_, Some(_)) => format!("the column [{}.{}]", parent_name, object_name),
            _ => format!("the [{}]", object_name),
        };
        return Err(super::rights::refuse(&format!(
            "Permission to grant or revoke one or more permissions on {named} is not granted."
        )));
    }
    Ok(grantable)
}

/// Grants or revokes rights to or from the built-in `DB:Public` (see
/// [`change_permission`]), which the engine keeps in the object itself
/// ([`dd::public_rights_slot`]) where it keeps other grantees' in Permission
/// objects (`fixtures/dictionary/public`). What it does:
/// - It refuses the grant option, the inherit bit and a column as it is told,
///   before it looks for the object.
/// - A grant adds to what the slot holds and a revoke takes from it, `ALL` being
///   the rights the object's type allows; a revoke where nothing is
///   granted yet writes nothing.
/// - Every write makes the slot anew, with a new random part, even when the
///   rights stay as they are, and a revoke of everything leaves the slot with no
///   rights.
/// - A slot that does not check is 5127, for a grant and a revoke alike.
fn change_public_permission(
    session: &Session,
    conn: &connection::Connection,
    dict: Dictionary,
    add_path: &Path,
    (object_type, object_name, parent_name): (u16, &str, &str),
    (rights, revoke, checked): (u32, bool, bool),
) -> Result<(), Error> {
    let all = rights == ALL || rights == ALL_WITH_GRANT;
    let base = rights & !permission::WITH_GRANT;
    if rights != ALL && rights & permission::WITH_GRANT != 0 {
        return Err(ace_error::sql(
            5158,
            "WITH GRANT permission is not valid for the DB:Public role.",
        ));
    }
    if !all && base & permission::INHERIT != 0 {
        return Err(ace_error::sql(5158, "Only user can inherit permissions."));
    }
    if object_type == 4 {
        return Err(ace_error::sql(
            5136,
            "Column level permission is not supported by the DB:Public role.",
        ));
    }
    let target = resolve_target(&dict, object_type, object_name, parent_name)?;
    if object_type == 0 && !all && base & !allowed(target.info) != 0 {
        return Err(ace_error::sql(5158, target.info.invalid));
    }
    let key = super::connection_key(conn, &dict).ok_or(Error::NotImplemented)?;
    // The slot holds rights on the table as a whole: what the user holds with
    // the grant option on some columns alone is not for it to give
    // (`fixtures/sql/ddpublicgrant`).
    let columns: Vec<String> = match target.info.object_type {
        1 => (dict.children(target.id, Kind::Field).iter())
            .map(|field| field.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let grantable = grantable_rights(
        conn,
        &target,
        (object_name, parent_name),
        (all, base),
        (checked, Some(&columns)),
    )?;
    let bits = if all {
        target.info.valid & grantable.unwrap_or(u32::MAX)
    } else {
        base
    };
    // `ALL` cut to nothing by what the user may grant writes no slot
    // (`fixtures/sql/ddpublicgrant`).
    if all && bits == 0 {
        return Ok(());
    }
    let object = dict.object(target.id).ok_or(Error::CorruptTable)?;
    let slot = dd::public_rights_slot(object.kind).ok_or(Error::CorruptTable)?;
    let held = match dict.public_rights(object, Some(&key)) {
        dd::PublicRights::Invalid => {
            return Err(ace_error::sql(
                5127,
                "Inconsistency detected while reading stored property.",
            ));
        }
        dd::PublicRights::Absent if revoke => return Ok(()),
        dd::PublicRights::Absent => 0,
        dd::PublicRights::Rights(held) => held,
    };
    let rights = if revoke { held & !bits } else { held | bits };
    let salt: [u8; 4] = super::login_salts().secret_first[..4].try_into().unwrap();
    let mut blob = crypto::public_rights_blob(rights, salt);
    crypto::crypt(
        dict.scheme(),
        &key,
        crypto::public_rights_item(object.id, slot as u32),
        &mut blob,
    );
    let record = object.record();
    drop(dict);
    write_object_slots(session, conn, add_path, record, &[(slot, Some(&blob))])
}

/// Grants or revokes `rights` on an object for `grantee`
/// (`AdsDDGrantPermission`/`AdsDDRevokePermission`). `object_type` 0 finds the
/// object by name among every kind; a column (type 4) is `object_name` of the
/// table `parent_name`; `ADS_PERMISSION_CREATE` (alone or with
/// `ADS_PERMISSION_INHERIT`) with no object name is the right to create objects
/// of the type (see [`change_create_permission`]).
///
/// The rules, probed:
/// - Every call raises the dictionary's version first, even one that then fails.
/// - A user's mask carries `ADS_PERMISSION_INHERIT` (inherit its groups' rights)
///   from the moment its Permission object is made; the bit is itself granted and
///   revoked like a right (not on a column, not to a group, not with the grant
///   option), and a grant of it alone where the user has no Permission object
///   writes nothing. `ALL` grants every applicable right and, to a user, the
///   inherit bit; `ALL_WITH_GRANT` the same with the grant option.
/// - A revoke carrying the grant option (`ALL_WITH_GRANT` included) takes away only
///   the grant option; a plain one the rights and their grant option alike; `ALL`
///   everything. A revoke with nothing granted still makes the Permission object.
/// - On a table, the rights of each column are kept in slot 2 (and those with the
///   grant option in slot 4) only while the columns differ; a right granted or
///   revoked on the table reaches every column, and slot 1 (slot 3) holds every
///   right any column holds.
/// - The with-grant slot 3 is written by a revoke only on a table (or column) or
///   where it is already set.
/// - Granting to another built-in group is 5136; to `DB:Public`, whose rights the
///   engine keeps in the object itself, see [`change_public_permission`].
#[allow(clippy::too_many_arguments)]
pub(super) fn change_permission(
    session: &Session,
    connection: Handle,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
    grantee: &str,
    rights: u32,
    revoke: bool,
) -> Result<(), Error> {
    change_permission_checked(
        session,
        connection,
        (object_type, object_name, parent_name),
        grantee,
        rights,
        revoke,
        true,
    )
}

/// [`change_permission`] for the rights a user gets on what it creates, which it
/// is given whatever it holds on them (there is nothing held yet).
pub(super) fn grant_to_creator(
    session: &Session,
    connection: Handle,
    object_type: u16,
    object_name: &str,
    grantee: &str,
    rights: u32,
) -> Result<(), Error> {
    change_permission_checked(
        session,
        connection,
        (object_type, object_name, ""),
        grantee,
        rights,
        false,
        false,
    )
}

/// Sets the `Info2` flags (the `0x8000_0000` every Permission object is made
/// with) of the Permission objects granted to the connection's user to none,
/// then rebuilds the index and reloads.
fn clear_permission_flags(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
) -> Result<(), Error> {
    let dict = open_dict(add_path)?;
    let user = conn
        .with_database_opt(|db| db.user().map(str::to_owned))
        .flatten()
        .and_then(|user| dict.named(Kind::User, &user).map(|u| u.id));
    let records: Vec<u32> = dict
        .all(Kind::Permission)
        .iter()
        .filter(|p| Some(p.parent) == user)
        .map(|p| p.record())
        .collect();
    drop(dict);
    if records.is_empty() {
        return Ok(());
    }
    {
        let mut file = super::open_dictionary_file(add_path)?;
        for record in records {
            file.write_part(record, dd::create::INFO2, &0u32.to_le_bytes())
                .map_err(|_| Error::CorruptTable)?;
        }
    }
    rebuild_dictionary(session, conn, add_path)
}

/// Grants or revokes as [`change_permission`] does, held to the grant option of
/// a user who is not the administrator only when `checked`.
fn change_permission_checked(
    session: &Session,
    connection: Handle,
    (object_type, object_name, parent_name): (u16, &str, &str),
    grantee: &str,
    rights: u32,
    revoke: bool,
    checked: bool,
) -> Result<(), Error> {
    let (conn, add_path) = dictionary_connection(session, connection)?;
    // Every call raises the version first, even one that then fails (and the
    // engine rewrites the index with it).
    if raise_permission_version(&add_path)? {
        rebuild_dictionary(session, &conn, &add_path)?;
    }
    // A member of `DB:Admin` that grants or revokes, whatever it asks for, takes
    // the flag of the Permission objects granted to it away
    // (`fixtures/sql/ddbuiltin`, `fixtures/sql/spddadminpw`).
    if checked
        && conn
            .with_database_opt(|db| db.is_admin_member())
            .unwrap_or(false)
    {
        clear_permission_flags(session, &conn, &add_path)?;
    }
    if object_type != 0 && type_info(object_type).is_none() {
        return Err(ace_error::sql(5158, "Parameter 2"));
    }
    let all = rights == ALL || rights == ALL_WITH_GRANT;
    let base = rights & !permission::WITH_GRANT;
    let with_grant = rights != ALL && rights & permission::WITH_GRANT != 0;
    if !all {
        if base == 0 {
            return Err(ace_error::sql(
                5158,
                "Parameter 6 - no permission is specified.",
            ));
        }
        if base & !DEFINED != 0 {
            return Err(ace_error::sql(5158, "Parameter 6"));
        }
        if base & CREATE != 0 {
            if base & !(CREATE | permission::INHERIT) != 0 {
                return Err(ace_error::sql(
                    5158,
                    "Parameter 6 - Create Permission not valid in conjunctuion with other object \
                     permissions.",
                ));
            }
            let inherit = base & permission::INHERIT != 0;
            return change_create_permission(
                session,
                &conn,
                &add_path,
                (object_type, object_name, parent_name),
                grantee,
                inherit,
                with_grant,
                revoke,
            );
        }
        if let Some(info) = type_info(object_type)
            && base & !allowed(info) != 0
        {
            return Err(ace_error::sql(5158, info.invalid));
        }
    }

    let dict = open_dict(&add_path)?;
    let Some(grantee) = resolve_grantee(&dict, grantee, |_| {
        ace_error::sql(
            5136,
            "Permission cannot be granted to or revoked from the specified database role.",
        )
    })?
    else {
        return change_public_permission(
            session,
            &conn,
            dict,
            &add_path,
            (object_type, object_name, parent_name),
            (rights, revoke, checked),
        );
    };
    let is_user = matches!(grantee, Grantee::User(_));
    if !all && base & permission::INHERIT != 0 {
        if !is_user {
            return Err(ace_error::sql(5158, "Only user can inherit permissions."));
        }
        if with_grant {
            return Err(ace_error::sql(
                5158,
                "Inherit permission cannot be granted with the WITH GRANT option.",
            ));
        }
    }
    let target = resolve_target(&dict, object_type, object_name, parent_name)?;
    if object_type == 0 && !all && base & !allowed(target.info) != 0 {
        return Err(ace_error::sql(5158, target.info.invalid));
    }
    let grantee_oid = grantee.object().id;
    if grantee_oid == target.id && target.column.is_none() {
        return Err(ace_error::sql(
            5158,
            "Users and user groups cannot be granted or revoke permissions to themselves.",
        ));
    }
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
    let user_inherit = if is_user { permission::INHERIT } else { 0 };
    let grantable = grantable_rights(
        &conn,
        &target,
        (object_name, parent_name),
        (all, base),
        (checked, None),
    )?;
    let bits = if all {
        let inherit = if target.column.is_some() {
            0
        } else {
            user_inherit
        };
        target.info.valid & grantable.unwrap_or(u32::MAX) | inherit
    } else {
        base
    };
    let column_bits = permission::column_byte(bits);
    // Only a table's (and so a column's) Permission object gets the with-grant
    // slot 3 from a revoke that finds it unset.
    let table_like = target.fields.is_some();

    let existing = dict
        .all(Kind::Permission)
        .into_iter()
        .find(|p| p.parent == grantee_oid && p.info1 == Some(target.id as i32))
        .map(|p| {
            let raw = |slot: usize| p.slot(slot).map(<[u8]>::to_vec);
            (p.record(), p.id, [raw(1), raw(2), raw(3), raw(4)])
        });
    // A grant of the inherit bit alone, which a user without a Permission object
    // holds already, writes nothing.
    if !revoke && existing.is_none() && is_user && !all && base == permission::INHERIT {
        return Ok(());
    }
    // An `ALL` cut to nothing a user who is not the administrator holds with the
    // grant option makes no Permission object.
    if all && grantable.is_some() && existing.is_none() && bits & !user_inherit == 0 {
        return Ok(());
    }
    let oid = match &existing {
        Some((_, id, _)) => *id,
        None => read_next_id(&add_path)?,
    };
    let raw = existing
        .as_ref()
        .map(|(_, _, raw)| raw.clone())
        .unwrap_or_default();

    let decipher = |slot: usize| -> Option<Vec<u8>> {
        let mut b = raw[slot - 1].clone()?;
        crypto::crypt(
            dict.scheme(),
            &key,
            crypto::permission_item(oid, slot as u32),
            &mut b,
        );
        Some(b)
    };
    let rights_of = |slot| decipher(slot).and_then(|b| crypto::permission_rights(&b));
    let mut s1 = match existing {
        Some(_) => rights_of(permission::OBJECT_RIGHTS).unwrap_or(0),
        None => user_inherit,
    };
    let mut s3 = rights_of(permission::WITH_GRANT_RIGHTS).unwrap_or(0);
    let s3_present = raw[2].is_some();
    // The per-column rights: the stored bytes, or the object rights reflected to
    // every column.
    let count = target.fields.unwrap_or(0);
    let columns_of = |slot: usize, mask: u32| -> Vec<u8> {
        match decipher(slot).and_then(|b| crypto::permission_column_rights(&b)) {
            Some(mut bytes) => {
                bytes.resize(count, 0);
                bytes
            }
            None => vec![permission::column_byte(mask); count],
        }
    };
    let mut c = columns_of(permission::COLUMN_RIGHTS, s1);
    let mut g = columns_of(permission::WITH_GRANT_COLUMN_RIGHTS, s3);
    let or_bytes = |cols: &[u8]| {
        cols.iter()
            .fold(0u32, |acc, &b| acc | permission::column_rights(b))
    };
    let col_less = |mask: u32| mask & !COLUMN_RIGHTS_MASK;
    match (target.column, revoke) {
        (Some(index), false) => {
            c[index] |= column_bits;
            s1 |= bits;
            if with_grant {
                g[index] |= column_bits;
                s3 |= bits;
            }
        }
        (None, false) => {
            s1 |= bits;
            c.iter_mut().for_each(|b| *b |= column_bits);
            if with_grant {
                s3 |= bits;
                g.iter_mut().for_each(|b| *b |= column_bits);
            }
        }
        (Some(index), true) => {
            if !with_grant {
                c[index] &= !column_bits;
                s1 = col_less(s1) | or_bytes(&c);
            }
            g[index] &= !column_bits;
            s3 = col_less(s3) | or_bytes(&g);
        }
        (None, true) => {
            if !with_grant {
                s1 &= !bits;
                c.iter_mut().for_each(|b| *b &= !column_bits);
            }
            s3 &= !bits;
            g.iter_mut().for_each(|b| *b &= !column_bits);
        }
    }

    // The slots to write: each the property index and the enciphered value (`None`
    // to drop it).
    let enc_rights = |rights: u32, slot: usize| -> Option<Vec<u8>> {
        let mut b = crypto::permission_blob(rights).to_vec();
        crypto::crypt(
            dict.scheme(),
            &key,
            crypto::permission_item(oid, slot as u32),
            &mut b,
        );
        Some(b)
    };
    // A per-column slot is kept only while the columns differ.
    let enc_columns = |cols: &[u8], slot: usize| -> Option<Vec<u8>> {
        if cols.windows(2).all(|w| w[0] == w[1]) {
            return None;
        }
        let mut b = crypto::permission_column_blob(cols);
        crypto::crypt(
            dict.scheme(),
            &key,
            crypto::permission_item(oid, slot as u32),
            &mut b,
        );
        Some(b)
    };
    let mut slots: Vec<(usize, Option<Vec<u8>>)> = vec![(
        permission::OBJECT_RIGHTS,
        enc_rights(s1, permission::OBJECT_RIGHTS),
    )];
    if table_like {
        slots.push((
            permission::COLUMN_RIGHTS,
            enc_columns(&c, permission::COLUMN_RIGHTS),
        ));
    }
    if s3_present || (with_grant && !revoke) || (revoke && table_like) {
        slots.push((
            permission::WITH_GRANT_RIGHTS,
            enc_rights(s3, permission::WITH_GRANT_RIGHTS),
        ));
    }
    if table_like {
        slots.push((
            permission::WITH_GRANT_COLUMN_RIGHTS,
            enc_columns(&g, permission::WITH_GRANT_COLUMN_RIGHTS),
        ));
    }

    if let Some((record, _, _)) = existing {
        // Rewrite only what changes.
        slots.retain(|(slot, value)| raw[slot - 1] != *value);
        if slots.is_empty() {
            return Ok(());
        }
        drop(dict);
        let refs: Vec<(usize, Option<&[u8]>)> =
            slots.iter().map(|(i, v)| (*i, v.as_deref())).collect();
        return write_object_slots(session, &conn, &add_path, record, &refs);
    }

    drop(dict);
    let name = format!("{:08X}", target.id);
    let slot_refs: Vec<(usize, &[u8])> = slots
        .iter()
        .filter_map(|(i, v)| v.as_deref().map(|v| (*i, v)))
        .collect();
    let record = dd::create::object_record(&NewObject {
        id: oid,
        parent: grantee_oid as i32,
        kind: "Permission",
        name: &name,
        info1: target.id as i32,
        info2: 0x8000_0000,
        slots: &slot_refs,
    });
    dd::create::insert_objects(&add_path, &[record], oid + 1).map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// Grants or revokes the right to create objects of `object_type` (`target`'s
/// first part; its name and parent must be empty) for `grantee`: one byte of the
/// grantee's create rights (its User or Group object's slot 6, see
/// [`permission::CREATE_RIGHTS`]). `inherit` (`ADS_PERMISSION_CREATE |
/// ADS_PERMISSION_INHERIT`, SQL `INHERIT CREATE …`) changes the user's inherit
/// bit for the right instead of the right itself. A grant sets the bit, and its
/// with-grant twin with the grant option; a revoke clears both, or with the grant
/// option only the twin. The slot is written whatever changes, even by a revoke
/// of nothing.
#[allow(clippy::too_many_arguments)]
fn change_create_permission(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    (object_type, object_name, parent_name): (u16, &str, &str),
    grantee: &str,
    inherit: bool,
    with_grant: bool,
    revoke: bool,
) -> Result<(), Error> {
    if !object_name.is_empty() {
        return Err(ace_error::sql(
            5132,
            "Parameter 3. Object name should be empty.",
        ));
    }
    if !parent_name.is_empty() {
        return Err(ace_error::sql(
            5132,
            "Parameter 4. Parent name should be empty.",
        ));
    }
    let index = permission::create_index(object_type).ok_or_else(|| {
        ace_error::sql(
            5136,
            "Create permission is not supported on the specified object type.",
        )
    })?;
    let dict = open_dict(add_path)?;
    let roles = |_: &str| {
        ace_error::sql(
            5132,
            "Create permissions of the specified database roles cannot be modified.",
        )
    };
    let grantee = resolve_grantee(&dict, grantee, roles)?.ok_or_else(|| roles("DB:Public"))?;
    let is_user = matches!(grantee, Grantee::User(_));
    if !is_user && inherit {
        return Err(ace_error::sql(
            5158,
            "Inherited permission is not valid for user group.",
        ));
    }
    if !conn
        .with_database_opt(|db| db.may_grant_create(object_type, with_grant && !revoke))
        .unwrap_or(true)
    {
        return Err(super::rights::refuse(
            "One or more permission cannot be granted or revoked.",
        ));
    }
    let key = super::connection_key(conn, &dict).ok_or(Error::NotImplemented)?;
    let object = grantee.object();
    let item = crypto::permission_item(object.id, permission::CREATE_RIGHTS as u32);
    let mut bytes = object
        .slot(permission::CREATE_RIGHTS)
        .and_then(|raw| {
            let mut b = raw.to_vec();
            crypto::crypt(dict.scheme(), &key, item, &mut b);
            crypto::permission_column_rights(&b)
        })
        .unwrap_or_else(|| {
            let fresh = if is_user {
                permission::CREATE_INHERIT
            } else {
                0
            };
            vec![fresh; permission::CREATE_RIGHTS_LENGTH]
        });
    let (bit, grant_bit) = match inherit {
        true => (
            permission::CREATE_INHERIT,
            permission::CREATE_INHERIT_WITH_GRANT,
        ),
        false => (permission::CREATE, permission::CREATE_WITH_GRANT),
    };
    let byte = &mut bytes[index];
    match (revoke, with_grant) {
        (false, false) => *byte |= bit,
        (false, true) => *byte |= bit | grant_bit,
        (true, false) => *byte &= !(bit | grant_bit),
        (true, true) => *byte &= !grant_bit,
    }
    let mut blob = crypto::permission_column_blob(&bytes);
    crypto::crypt(dict.scheme(), &key, item, &mut blob);
    let record = object.record();
    drop(dict);
    write_object_slots(
        session,
        conn,
        add_path,
        record,
        &[(permission::CREATE_RIGHTS, Some(&blob))],
    )
}

/// Runs SQL `GRANT` or `REVOKE` (`grant`) on the dictionary `connection` is to,
/// as the original does (probed, `fixtures/dictionary/grants.json`): each
/// permission in turn is one `AdsDDGrantPermission`/`AdsDDRevokePermission` —
/// a right on the `ON` object found by name among every kind (object type 0), a
/// column's (type 4, the object its table), `ALL`, the database's (type 11) or
/// the right to create a kind — and the first that fails ends the statement,
/// keeping those before it. A permission on an object with no `ON` (beside a
/// database-wide one) fails as an empty object name.
pub fn sql_grant(
    session: &Session,
    connection: Handle,
    grant: &tflip_sql::ast::Grant,
) -> Result<(), Error> {
    use tflip_sql::ast::Permission;
    let option = if grant.grant_option {
        permission::WITH_GRANT
    } else {
        0
    };
    for item in &grant.permissions {
        let object = grant.object.as_deref();
        let on = || object.ok_or_else(empty_object_name);
        let (object_type, name, parent, rights) = match item {
            Permission::Object {
                right,
                column: None,
            } => (0, on()?, "", right | option),
            Permission::Object {
                right,
                column: Some(column),
            } => (4, column.as_str(), on()?, right | option),
            Permission::All => (
                0,
                on()?,
                "",
                if grant.grant_option {
                    ALL_WITH_GRANT
                } else {
                    ALL
                },
            ),
            Permission::Database(right) => (11, "", "", right | option),
            Permission::Create {
                object_type,
                inherit,
            } => {
                let inherit = if *inherit { permission::INHERIT } else { 0 };
                (*object_type, "", "", CREATE | inherit | option)
            }
        };
        change_permission(
            session,
            connection,
            object_type,
            name,
            parent,
            &grant.grantee,
            rights,
            grant.revoke,
        )?;
    }
    Ok(())
}
