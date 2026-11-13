//! Users, groups, memberships and permissions.

use super::*;

/// Creates a user group in the data dictionary `connection` is to
/// (`AdsDDCreateUserGroup`): a Group object under the Database, with the
/// description (if any) stored in the dictionary memo file, written as the
/// original engine writes it — then the index is rebuilt and the connection
/// reloaded. The name must not be empty, a built-in group's, or an existing
/// group's (`AE_INVALID_OBJECT_NAME`, 5132).
pub fn create_group_api(
    session: &Session,
    connection: Handle,
    name: &str,
    description: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::rights::require_create(&conn, 9, "user group")?;
    if name.is_empty() || dd::built_in_group(name).is_some() {
        return Err(invalid_object_name(name));
    }
    let dict = open_dict(&add_path)?;
    if dict.named(Kind::Group, name).is_some() {
        return Err(invalid_object_name(name));
    }
    drop(dict);

    const DATABASE_ID: i32 = 1;
    let next_id = read_next_id(&add_path)?;
    let mut record = dd::create::object_record(&NewObject {
        id: next_id,
        parent: DATABASE_ID,
        kind: "Group",
        name,
        info1: i32::MIN, // 0x80000000, as the engine writes it
        info2: 0x8000_0000,
        slots: &[],
    });
    dd::create::set_description(
        &mut record,
        &dd::memo_path(&add_path),
        description.as_bytes(),
    )
    .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)?;
    super::rights::grant_creator(session, connection, 9, name)
}

/// Deletes a user group from the data dictionary `connection` is to
/// (`AdsDDDeleteUserGroup`): soft-deletes its Group object and rebuilds the
/// index, as the original engine does for a group with no members. A built-in
/// group or a name that is no group fails with `AE_INVALID_OBJECT_NAME` (5132).
pub fn delete_group_api(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if dd::built_in_group(name).is_some() {
        return Err(invalid_object_name(name));
    }
    let dict = open_dict(&add_path)?;
    let group = find_named_api(&dict, Kind::Group, "user group", name)?;
    super::rights::require(&conn, 9, name, super::rights::DROP, "")?;
    let record = group.record();
    let slots = with_grants(&dict, group.id, record);
    drop(dict);

    delete_records(session, &conn, &add_path, &slots)
}

/// Creates a user in the data dictionary `connection` is to
/// (`AdsDDCreateUser`): a User object under the Database, with the login secret
/// that authenticates `password` (slot 0, obfuscated at the user's record as
/// the original engine stores it), an empty Internet-access slot, and the
/// description (if any) in the dictionary memo. The user joins only the
/// built-in `DB:Public` group; putting a user in another group (a non-empty
/// `group`) needs the group-membership encryption, which is not written yet
/// (`Error::NotImplemented`). The name must not be empty, a built-in group's or
/// an existing user's (`AE_INVALID_OBJECT_NAME`, 5132).
pub fn create_user_api(
    session: &Session,
    connection: Handle,
    group: &str,
    name: &str,
    password: &str,
    description: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::rights::require_create(&conn, 8, "user")?;
    if name.is_empty() || dd::built_in_group(name).is_some() {
        return Err(invalid_object_name(name));
    }
    let dict = open_dict(&add_path)?;
    if dict.named(Kind::User, name).is_some() {
        return Err(invalid_object_name(name));
    }

    // The group the user joins: DB:Public (empty or named) adds no membership
    // slot; a named group is encrypted into slot 1 with the dictionary key.
    let membership = if group.is_empty() || group.eq_ignore_ascii_case("DB:Public") {
        None
    } else {
        let object = dict
            .named(Kind::Group, group)
            .ok_or(Error::NotImplemented)?;
        let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
        let mut slot = object.id.to_le_bytes().to_vec();
        tflip_storage::crypto::group_membership(dict.scheme(), &key, &mut slot);
        Some(slot)
    };
    let scheme = dict.scheme();
    drop(dict);

    const DATABASE_ID: i32 = 1;
    let next_id = read_next_id(&add_path)?;
    // The new record goes in the next slot, a deleted object's if there is one;
    // its 1-based record number salts the obfuscation of the login secret.
    let record_number = tflip_storage::dd::DictionaryFile::open(&add_path)
        .map_err(|_| Error::CorruptTable)?
        .next_slot();
    let mut secret = user_secret(&conn, password.as_bytes());
    tflip_storage::crypto::obfuscate(scheme, &mut secret, record_number, 0);

    let empty: &[u8] = &[];
    let mut slots: Vec<(usize, &[u8])> = vec![(dd::user::PASSWORD_SECRET, &secret)];
    if let Some(slot) = &membership {
        slots.push((dd::user::GROUP_MEMBERSHIP, slot));
    }
    slots.push((dd::user::ENABLE_INTERNET, empty));
    let mut record = dd::create::object_record(&NewObject {
        id: next_id,
        parent: DATABASE_ID,
        kind: "User",
        name,
        info1: i32::from(membership.is_some()), // the number of groups beyond DB:Public
        info2: 0x8000_0000,
        slots: &slots,
    });
    dd::create::set_description(
        &mut record,
        &dd::memo_path(&add_path),
        description.as_bytes(),
    )
    .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)?;
    super::rights::grant_creator(session, connection, 8, name)
}

/// A user's login secret for `password`: it holds the dictionary key, as
/// the original engine's do, so that the user's login finds it, when the
/// connection knows the key (`fixtures/sql/spddusers`).
pub(crate) fn user_secret(conn: &connection::Connection, password: &[u8]) -> Vec<u8> {
    let salts = login_salts();
    let password = &password[..password.len().min(20)];
    let mut last = salts.secret_last;
    let scheme = conn.with_database_opt(|db| db.scheme()).unwrap_or_default();
    if let Some(key) = conn.with_database_opt(|db| db.key()).flatten()
        && key.len() == last.len()
    {
        last.copy_from_slice(&key);
        tflip_storage::crypto::wrap(scheme, password, &mut last);
    }
    tflip_storage::crypto::make_secret(scheme, password, &salts.secret_first, &last)
}

/// Gives the Database object's login slot the secret of `password` (at most
/// 20 bytes) for the administrator's login. The dictionary key stays what it
/// is, as the original keeps it (`fixtures/dictionary/adminpw`): the secret
/// holds it for the new password, so no membership, right or table password
/// is enciphered again. The wrapped table password in front of the secret is
/// kept.
pub(crate) fn write_admin_password(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    password: &[u8],
) -> Result<(), Error> {
    use tflip_storage::crypto;
    let password = &password[..password.len().min(20)];
    let dict = open_dict(add_path)?;
    let database = dict.database().ok_or(Error::CorruptTable)?;
    let mut slot = database.slot(1).unwrap_or(&[]).to_vec();
    crypto::obfuscate(dict.scheme(), &mut slot, database.record(), 1);
    let wrapped = slot.first().map_or(0, |&n| 1 + usize::from(n));
    let mut value = slot[..wrapped.min(slot.len())].to_vec();
    if value.is_empty() {
        value.push(0);
    }
    value.extend(user_secret(conn, password));
    crypto::obfuscate(dict.scheme(), &mut value, database.record(), 1);
    let record = database.record();
    drop(dict);
    // The reload after the write logs in again, with the new password.
    conn.set_database_password(password);
    set_object_slot(session, conn, add_path, record, 1, &value)
}

/// Sets the administrator's password (`ADS_DD_ADMIN_PASSWORD`, and
/// `sp_ModifyDatabase`'s `ADMIN_PASSWORD`), given as the property's bytes: a
/// string up to its NUL, which the property's length includes, or no bytes
/// for none (`fixtures/dictionary/adminpw`). Only `ADSSYS` may change it,
/// not a member of `DB:Admin` either (5054), and the buffer is checked first.
pub(crate) fn set_admin_password(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    value: &[u8],
) -> Result<(), Error> {
    if value.last().is_some_and(|&last| last != 0) {
        return Err(ace_error::sql(
            5133,
            "Property length must include the NULL terminator.",
        ));
    }
    let own = conn
        .with_database_opt(|db| db.user().is_none_or(|u| u.eq_ignore_ascii_case("ADSSYS")))
        .unwrap_or(true);
    if !own {
        return Err(ace_error::sql(
            5054,
            "ADSSYS password can only be changed by ADSSYS.",
        ));
    }
    let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
    write_admin_password(session, conn, add_path, &value[..end])
}

/// What the connection's user is, for who may change whose password: whether
/// it is the administrator (or logged in as no one), a member of `DB:Admin`,
/// and its name.
struct Caller {
    name: Option<String>,
    administrator: bool,
    admin_member: bool,
}

fn caller(conn: &connection::Connection) -> Caller {
    conn.with_database_opt(|db| Caller {
        name: db.user().map(str::to_owned),
        administrator: db.user().is_none() || db.is_admin(),
        admin_member: db.is_admin_member(),
    })
    .unwrap_or(Caller {
        name: None,
        administrator: true,
        admin_member: false,
    })
}

/// Sets the password of the user `target` as `AdsDDSetUserProperty`'s
/// `ADS_DD_USER_PASSWORD` and `sp_ModifyUserProperty` do (at most 20 bytes of
/// `new`): the administrator's only by the administrator, a user's own only by a
/// user not made to give the old one (`REQUIRE_OLD_PASSWORD`, by default it is),
/// and another's by the administrator, a member of `DB:Admin` or a user
/// holding `ALTER` on it (`fixtures/sql/spddpwchange`).
pub(crate) fn set_user_password(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    target: &str,
    new: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let who = caller(conn);
    if target.eq_ignore_ascii_case("ADSSYS") {
        return match (who.administrator, who.admin_member) {
            (true, _) => write_admin_password(session, conn, add_path, new),
            (false, true) => Err(ace_error::sql(
                5054,
                "ADSSYS password can only be changed by ADSSYS.",
            )),
            (false, false) => Err(super::rights::refuse("")),
        };
    }
    let own = who
        .name
        .as_deref()
        .is_some_and(|n| n.eq_ignore_ascii_case(target));
    let (record, requires_old) = {
        let dict = open_dict(add_path)?;
        let Some(user) = dict.named(Kind::User, target) else {
            return Err(not_valid(
                "There is no such named user in the data dictionary.",
            ));
        };
        (
            user.record(),
            user.slot(dd::user::NO_OLD_PASSWORD).is_none(),
        )
    };
    if own && !who.administrator {
        if requires_old {
            return Err(super::rights::refuse(
                "The old password is needed to change current user's password.",
            ));
        }
    } else if !own && !super::rights::may(conn, 8, target, super::rights::ALTER) {
        return Err(super::rights::refuse(""));
    }
    let new = &new[..new.len().min(20)];
    let mut secret = user_secret(conn, new);
    tflip_storage::crypto::obfuscate(
        conn.with_database_opt(|db| db.scheme()).unwrap_or_default(),
        &mut secret,
        record,
        0,
    );
    if own {
        conn.set_database_password(new);
    }
    set_object_slot(
        session,
        conn,
        add_path,
        record,
        dd::user::PASSWORD_SECRET,
        &secret,
    )
}

/// Changes the connection's own password, giving the old one
/// (`ADS_DD_CURRENT_USER_PASSWORD`, `sp_ChangeCurrentUserPassword`): a user
/// who needs the old password gives it (5097 for another), the administrator
/// never does (`fixtures/sql/spddusers`, `spddpwchange`).
pub(crate) fn change_own_password(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    old: &[u8],
    new: &[u8],
) -> Result<(), Error> {
    use tflip_storage::{crypto, dd};
    let (old, new) = (&old[..old.len().min(20)], &new[..new.len().min(20)]);
    let who = caller(conn);
    let wrong_old = || ace_error::sql(5097, "The old password is not valid.");
    let Some(user) = who.name.filter(|user| !user.eq_ignore_ascii_case("ADSSYS")) else {
        return write_admin_password(session, conn, add_path, new);
    };
    let dict = open_dict(add_path)?;
    let object = dict.named(dd::Kind::User, &user).ok_or_else(wrong_old)?;
    let secret = object.slot(dd::user::PASSWORD_SECRET).unwrap_or(&[]);
    let required = object.slot(dd::user::NO_OLD_PASSWORD).is_none();
    if required && !crypto::user_password_ok(dict.scheme(), secret, object.record(), 0, old) {
        return Err(wrong_old());
    }
    let record = object.record();
    let mut value = user_secret(conn, new);
    crypto::obfuscate(dict.scheme(), &mut value, record, 0);
    drop(dict);
    conn.set_database_password(new);
    set_object_slot(
        session,
        conn,
        add_path,
        record,
        dd::user::PASSWORD_SECRET,
        &value,
    )
}

/// Deletes a user from the data dictionary `connection` is to
/// (`AdsDDDeleteUser`): soft-deletes its User object, freeing its description
/// memo, and rebuilds the index. A name that is no user fails with
/// `AE_INVALID_OBJECT_NAME` (5132).
pub fn delete_user_api(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let Some(user) = dict.named(Kind::User, name) else {
        return Err(not_valid("The specified user does not exist."));
    };
    super::rights::require(
        &conn,
        8,
        name,
        super::rights::DROP,
        "Permission to drop the user is not granted.",
    )?;
    let record = user.record();
    let slots = with_grants(&dict, user.id, record);
    drop(dict);

    delete_records(session, &conn, &add_path, &slots)
}

/// The record of a user or group and then the Permission objects on it and
/// those it holds, which go with it, in the order the original deletes them
/// (`fixtures/sql/ddslots`, `ddrights`).
pub(super) fn with_grants(dict: &tflip_storage::dd::Dictionary, id: u32, record: u32) -> Vec<u32> {
    let all = dict.all(tflip_storage::dd::Kind::Permission);
    let on = all.iter().filter(|p| p.info1 == i32::try_from(id).ok());
    let held = all.iter().filter(|p| p.parent == id);
    std::iter::once(record)
        .chain(on.chain(held).map(|p| p.record()))
        .collect()
}

/// Adds the user `user` to the group `group` in the data dictionary
/// `connection` is to (`AdsDDAddUserToGroup`): decrypts the user's current group
/// membership with the dictionary key, appends the group's object id, and writes
/// it back encrypted (property slot 1), with the group count in `Info1`, as the
/// original engine does. A user already in the group fails with 5157; a missing
/// user or group with `AE_INVALID_OBJECT_NAME` (5132); a built-in group, or a
/// dictionary whose key tflip cannot recover (a non-empty admin password), with
/// `Error::NotImplemented`.
pub fn add_user_to_group_api(
    session: &Session,
    connection: Handle,
    group: &str,
    user: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    // A group that is none is refused before the right to alter it is.
    if dd::built_in_group(group).is_none()
        && open_dict(&add_path)?.named(Kind::Group, group).is_none()
    {
        return Err(not_named("user group"));
    }
    if !super::rights::may(&conn, 9, group, super::rights::ALTER) {
        return Err(super::rights::refuse(""));
    }
    if let Some(bit) = dd::built_in_membership_bit(group) {
        return change_built_in_membership(session, &conn, &add_path, user, bit, true);
    }
    if dd::built_in_group(group).is_some() {
        return Err(Error::NotImplemented);
    }
    let dict = open_dict(&add_path)?;
    let group = dict
        .named(Kind::Group, group)
        .ok_or_else(|| invalid_object_name(group))?;
    let user_obj = find_named_api(&dict, Kind::User, "user", user)?;
    let record = user_obj.record();
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
    let scheme = dict.scheme();
    let mut ids = decode_membership(scheme, user_obj, &key);
    let id = group.id;
    if ids.contains(&id) {
        return Err(ace_error::sql(5157, &format!("User: {user}")));
    }
    ids.push(id);
    drop(dict);
    write_user_membership(session, &conn, &add_path, record, &ids, scheme, &key)
}

/// Removes the user `user` from the group `group` in the data dictionary
/// `connection` is to (`AdsDDRemoveUserFromGroup`): decrypts the user's group
/// membership, drops the group's object id, and writes it back (property slot 1
/// goes away when no group is left), with the group count in `Info1`, as the
/// original engine does. A user not in the group fails with 5156; a missing user
/// or group with `AE_INVALID_OBJECT_NAME` (5132); a built-in group, or a
/// dictionary whose key tflip cannot recover, with `Error::NotImplemented`.
pub fn remove_user_from_group_api(
    session: &Session,
    connection: Handle,
    group: &str,
    user: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    // A group that is none is refused before the right to alter it is.
    if dd::built_in_group(group).is_none()
        && open_dict(&add_path)?.named(Kind::Group, group).is_none()
    {
        return Err(not_named("user group"));
    }
    if !super::rights::may(&conn, 9, group, super::rights::ALTER) {
        return Err(super::rights::refuse(""));
    }
    if let Some(bit) = dd::built_in_membership_bit(group) {
        return change_built_in_membership(session, &conn, &add_path, user, bit, false);
    }
    if dd::built_in_group(group).is_some() {
        return Err(Error::NotImplemented);
    }
    let dict = open_dict(&add_path)?;
    let group = dict
        .named(Kind::Group, group)
        .ok_or_else(|| invalid_object_name(group))?;
    let user_obj = find_named_api(&dict, Kind::User, "user", user)?;
    let record = user_obj.record();
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
    let scheme = dict.scheme();
    let mut ids = decode_membership(scheme, user_obj, &key);
    let id = group.id;
    if !ids.contains(&id) {
        return Err(ace_error::sql(5156, &format!("User: {user}")));
    }
    ids.retain(|&g| g != id);
    drop(dict);
    write_user_membership(session, &conn, &add_path, record, &ids, scheme, &key)
}

/// Adds the user `user` to (`add`) or takes it from the built-in group whose bit
/// in the membership mask is `bit`, as `AdsDDAddUserToGroup` and
/// `AdsDDRemoveUserFromGroup` do: the mask in the user's property slot 8,
/// enciphered with the dictionary key, written whole with new random parts on
/// every change (`fixtures/sql/ddbuiltin`). The slot stays when the last group
/// goes.
fn change_built_in_membership(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    user: &str,
    bit: u32,
    add: bool,
) -> Result<(), Error> {
    use tflip_storage::crypto;
    use tflip_storage::dd::{self, Kind};
    let dict = open_dict(add_path)?;
    let user_obj = find_named_api(&dict, Kind::User, "user", user)?;
    let key = super::connection_key(conn, &dict).ok_or(Error::NotImplemented)?;
    let item = crypto::built_in_membership_item(user_obj.id);
    let mask = user_obj
        .slot(dd::user::BUILT_IN_MEMBERSHIP)
        .and_then(|slot| {
            let mut data = slot.to_vec();
            crypto::crypt(dict.scheme(), &key, item, &mut data);
            crypto::built_in_membership_mask(&data)
        })
        .unwrap_or(0);
    match (add, mask & bit != 0) {
        (true, true) => return Err(ace_error::sql(5157, &format!("User: {user}"))),
        (false, false) => return Err(ace_error::sql(5156, &format!("User: {user}"))),
        _ => {}
    }
    let salts = login_salts();
    let salt: [u8; 4] = salts.secret_first[..4].try_into().unwrap();
    let mut blob = crypto::built_in_membership_blob(mask ^ bit, salt);
    crypto::crypt(dict.scheme(), &key, item, &mut blob);
    let record = user_obj.record();
    drop(dict);
    write_object(
        session,
        conn,
        add_path,
        record,
        &[(dd::user::BUILT_IN_MEMBERSHIP, Some(&blob))],
        None,
    )
}

/// A user's group object ids, decrypted from its membership slot with the
/// dictionary `key`; empty for a user in no group but the built-in `DB:Public`.
pub(crate) fn decode_membership(
    scheme: tflip_storage::crypto::Scheme,
    user: &tflip_storage::dd::Object,
    key: &[u8],
) -> Vec<u32> {
    use tflip_storage::dd::user::GROUP_MEMBERSHIP;
    let Some(slot) = user.slot(GROUP_MEMBERSHIP) else {
        return Vec::new();
    };
    let mut data = slot.to_vec();
    tflip_storage::crypto::group_membership(scheme, key, &mut data);
    data.chunks(4)
        .filter(|c| c.len() == 4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// Writes the user at record `record`'s group membership: the group `ids`
/// encrypted with the dictionary `key` into property slot 1 (the slot goes away
/// when `ids` is empty), and the group count into `Info1`, then rebuilds the
/// index and reloads.
pub(crate) fn write_user_membership(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    record: u32,
    ids: &[u32],
    scheme: tflip_storage::crypto::Scheme,
    key: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd;
    let data = (!ids.is_empty()).then(|| {
        let mut data: Vec<u8> = ids.iter().flat_map(|id| id.to_le_bytes()).collect();
        tflip_storage::crypto::group_membership(scheme, key, &mut data);
        data
    });
    write_object(
        session,
        conn,
        add_path,
        record,
        &[(dd::user::GROUP_MEMBERSHIP, data.as_deref())],
        Some(ids.len() as i32),
    )
}

/// 5158, `AE_INVALID_OBJECT_RIGHT`: a grant carries bits the object of the
/// kind `kind` does not allow, as the grants word it
/// (`tools/ace/dd/setobj.c`).
fn invalid_object_right(kind: tflip_storage::dd::Kind) -> Error {
    permissions::invalid_right(kind)
}

/// Grants `rights` on the object `object_name` (of `object_type`) to `grantee` (a
/// user or group) in the data dictionary `connection` is to
/// (`AdsDDGrantPermission`): a Permission object under the grantee holding the
/// rights mask, enciphered with the dictionary key, as the original engine writes
/// it. A second grant to the same grantee on the same object accumulates into the
/// one object. Object type 0 finds the object by name among every kind; the right
/// to create (`ADS_PERMISSION_CREATE`, no object name) lives in the grantee's own
/// object. A dictionary whose key cannot be recovered is `Error::NotImplemented`;
/// see `permissions::change_permission` for the rules.
pub fn grant_permission_api(
    session: &Session,
    connection: Handle,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
    grantee: &str,
    rights: u32,
) -> Result<(), Error> {
    permissions::change_permission(
        session,
        connection,
        object_type,
        object_name,
        parent_name,
        grantee,
        rights,
        false,
    )
}

/// Revokes `rights` on the object `object_name` from `grantee`
/// (`AdsDDRevokePermission`): clears the bits in the grantee's Permission object
/// for it — only the grant option when `rights` carries
/// `ADS_PERMISSION_WITH_GRANT` — keeping the object (as the engine does, even with
/// no right left, and making one if there was none). See [`grant_permission_api`]
/// for the limits.
pub fn revoke_permission_api(
    session: &Session,
    connection: Handle,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
    grantee: &str,
    rights: u32,
) -> Result<(), Error> {
    permissions::change_permission(
        session,
        connection,
        object_type,
        object_name,
        parent_name,
        grantee,
        rights,
        true,
    )
}

/// Reads the permission mask `grantee` holds on an object (`AdsDDGetPermissions`):
/// the bits granted or held implicitly, plus the inherit bit for a user.
/// `inherited` (`usGetInherited`) adds the grants of the groups a user belongs to.
/// See [`crate::system::permissions_mask`] for the rules.
pub fn get_permissions_api(
    session: &Session,
    connection: Handle,
    grantee: &str,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
    inherited: bool,
) -> Result<u32, Error> {
    let conn = connection::get(session, connection)?;
    conn.with_database_opt(|db| {
        crate::system::permissions_mask(
            db,
            grantee,
            object_type,
            object_name,
            parent_name,
            inherited,
        )
    })
    .ok_or(Error::NoDictionary)?
}

/// The rights a column grant may carry: read, update and insert.
pub(super) const COLUMN_RIGHTS_MASK: u32 = 0x0001 | 0x0002 | 0x0010;

/// `ADS_PERMISSION_*` bits used to validate grants on the object types other than
/// tables and columns.
mod perm_bit {
    pub const EXECUTE: u32 = 0x0004;
    pub const LINK_ACCESS: u32 = 0x0040;
    pub const ALTER: u32 = 0x0100;
    pub const DROP: u32 = 0x0200;
}

/// The rights an `AdsDDSetObjectAccessRights` access string can set, by letter
/// (case-insensitive), as probed from the original engine: `R` read, `W` the
/// write rights (update, insert and delete together), `X` execute, `I` inherit.
/// Every other character is ignored. The result is validated against the
/// object's type below.
fn access_string_rights(access: &str) -> u32 {
    use tflip_storage::dd::permission;
    let mut bits = 0u32;
    for ch in access.bytes() {
        bits |= match ch.to_ascii_uppercase() {
            b'R' => permission::READ,
            b'W' => permission::UPDATE | permission::INSERT | permission::DELETE,
            b'X' => permission::EXECUTE,
            b'I' => permission::INHERIT,
            _ => 0,
        };
    }
    bits
}

/// 5137, the dictionary has no object of the name given (of any type).
fn no_object_found() -> Error {
    ace_error::sql(5137, "The given object was not found.")
}

/// Sets `accessor`'s rights on the object named `object_name` to exactly the
/// rights the `allowed_access` string spells (`AdsDDSetObjectAccessRights`). The
/// object's type is inferred from its name (the call carries none); its rights
/// go into the accessor's Permission object for it, enciphered, replacing
/// whatever was there (unlike a grant, which merges). A right the object's type
/// cannot carry is refused with 5158, an unknown object with 5137 and an unknown
/// accessor with 5132. A dictionary whose key tflip cannot recover (a non-empty
/// administrator password) fails as not implemented.
pub fn set_object_access_rights_api(
    session: &Session,
    connection: Handle,
    object_name: &str,
    accessor_name: &str,
    allowed_access: &str,
) -> Result<(), Error> {
    use perm_bit::{ALTER, DROP, EXECUTE, LINK_ACCESS};
    use tflip_storage::crypto;
    use tflip_storage::dd::{self, Kind, create::NewObject, permission};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let bits = access_string_rights(allowed_access);

    let dict = open_dict(&add_path)?;
    // Infer the object's type by looking its name up, and with it the rights that
    // type can carry (its applicable permissions, as for a grant).
    let find = |kind, valid: u32| {
        dict.named_in_database(kind, object_name)
            .map(|o| (o.id, valid, kind))
    };
    let (target_oid, valid, kind) = find(Kind::Table, permission::TABLE_RIGHTS)
        .or_else(|| find(Kind::View, permission::TABLE_RIGHTS))
        .or_else(|| find(Kind::Procedure, EXECUTE | ALTER | DROP))
        .or_else(|| find(Kind::Function, EXECUTE | ALTER | DROP))
        .or_else(|| find(Kind::Package, EXECUTE | ALTER | DROP))
        .or_else(|| find(Kind::User, ALTER | DROP))
        .or_else(|| find(Kind::Group, ALTER | DROP))
        .or_else(|| find(Kind::Link, LINK_ACCESS | ALTER | DROP))
        .ok_or_else(no_object_found)?;
    // The inherit bit is always allowed; any other bit outside the type's set is
    // not (the engine returns 5158).
    if bits & !(valid | permission::INHERIT) != 0 {
        return Err(invalid_object_right(kind));
    }
    let grantee_oid = if let Some(user) = dict.named(Kind::User, accessor_name) {
        user.id
    } else if let Some(group) = dict.named(Kind::Group, accessor_name) {
        group.id
    } else {
        return Err(invalid_object_name(accessor_name));
    };
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;

    let existing = dict
        .all(Kind::Permission)
        .into_iter()
        .find(|p| p.parent == grantee_oid && p.info1 == Some(target_oid as i32))
        .map(|p| (p.record(), p.id));
    let scheme = dict.scheme();
    let enc_rights = |oid: u32, rights: u32| -> Vec<u8> {
        let mut b = crypto::permission_blob(rights).to_vec();
        crypto::crypt(
            scheme,
            &key,
            crypto::permission_item(oid, permission::OBJECT_RIGHTS as u32),
            &mut b,
        );
        b
    };

    if let Some((record, oid)) = existing {
        drop(dict);
        let blob = enc_rights(oid, bits);
        return set_object_slots(
            session,
            &conn,
            &add_path,
            record,
            &[(permission::OBJECT_RIGHTS, &blob)],
        );
    }
    raise_permission_version(&add_path)?;
    let next_id = read_next_id(&add_path)?;
    drop(dict);
    let blob = enc_rights(next_id, bits);
    let name = format!("{target_oid:08X}");
    let record = dd::create::object_record(&NewObject {
        id: next_id,
        parent: grantee_oid as i32,
        kind: "Permission",
        name: &name,
        info1: target_oid as i32,
        info2: 0x8000_0000,
        slots: &[(permission::OBJECT_RIGHTS, &blob)],
    });
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn creates_and_deletes_groups_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();

        // Create: a fresh dictionary, then four groups with descriptions of
        // telling lengths (empty, 8, 15, 17) — the engine's AdsDDCreateUserGroup.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddgroup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_group_api(&session, connect, "Managers", "people who manage").unwrap();
        create_group_api(&session, connect, "Staff", "").unwrap();
        create_group_api(&session, connect, "Eight", "exactly8").unwrap();
        create_group_api(&session, connect, "Sixteen", "sixteen_charsxx").unwrap();
        for (file, golden) in [
            ("db.add", "group.add"),
            ("db.am", "group.am"),
            ("db.ai", "group.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_login(ours), mask_login(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "create: {file}");
        }
        // A reserved, empty, or already-used name is refused (5132), nothing
        // written.
        for name in ["Managers", "MANAGERS", "", "DB:Admin"] {
            assert!(matches!(
                create_group_api(&session, connect, name, "x"),
                Err(Error::Sql(_))
            ));
        }
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);

        // Delete: dict + two groups, then delete one — AdsDDDeleteUserGroup.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddgroupdel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_group_api(&session, connect, "Managers", "people who manage").unwrap();
        create_group_api(&session, connect, "Staff", "the staff group").unwrap();
        delete_group_api(&session, connect, "Staff").unwrap();
        for (file, golden) in [
            ("db.add", "groupdel.add"),
            ("db.am", "groupdel.am"),
            ("db.ai", "groupdel.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_login(ours), mask_login(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "delete: {file}");
        }
        // Deleting a built-in group or a name that is no group is refused.
        assert!(matches!(
            delete_group_api(&session, connect, "DB:Admin"),
            Err(Error::Sql(_))
        ));
        assert!(matches!(
            delete_group_api(&session, connect, "Nope"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_and_deletes_users_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();

        // Create: a fresh dictionary, then two users in no group (DB:Public) —
        // one with a password and description, one with neither.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-dduser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_user_api(&session, connect, "", "alice", "secret", "Alice A").unwrap();
        create_user_api(&session, connect, "", "carol", "", "").unwrap();
        for (file, golden) in [
            ("db.add", "user.add"),
            ("db.am", "user.am"),
            ("db.ai", "user.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_user_secrets(ours), mask_user_secrets(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "create: {file}");
        }
        // The login secret tflip wrote authenticates the password and rejects a
        // wrong one, decoded as the engine decodes it (obfuscated at the user's
        // record, property slot 0).
        {
            use tflip_storage::dd::{Dictionary, Kind};
            let dict = Dictionary::open(&add).unwrap();
            let alice = dict.named(Kind::User, "alice").unwrap();
            let secret = alice.slot(0).unwrap();
            let ok = tflip_storage::crypto::decode_user_secret(
                dict.scheme(),
                secret,
                alice.record(),
                0,
                b"secret",
            );
            assert!(ok.is_some_and(|s| s.password_ok), "alice's secret verifies");
            let bad = tflip_storage::crypto::decode_user_secret(
                dict.scheme(),
                secret,
                alice.record(),
                0,
                b"nope",
            );
            assert!(
                bad.is_some_and(|s| !s.password_ok),
                "a wrong password is rejected"
            );
        }

        // A user in a real group is not written yet; names are validated.
        assert_eq!(
            create_user_api(&session, connect, "DB:Admin", "eve", "p", ""),
            Err(Error::NotImplemented)
        );
        for name in ["alice", "ALICE", "", "DB:Public"] {
            assert!(matches!(
                create_user_api(&session, connect, "", name, "p", ""),
                Err(Error::Sql(_))
            ));
        }
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);

        // Delete: dict + two users, then delete one.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-dduserdel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_user_api(&session, connect, "", "alice", "secret", "Alice A").unwrap();
        create_user_api(&session, connect, "", "carol", "", "").unwrap();
        delete_user_api(&session, connect, "carol").unwrap();
        for (file, golden) in [
            ("db.add", "userdel.add"),
            ("db.am", "userdel.am"),
            ("db.ai", "userdel.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_user_secrets(ours), mask_user_secrets(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "delete: {file}");
        }
        assert!(matches!(
            delete_user_api(&session, connect, "nobody"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adds_and_removes_group_membership_like_the_engine() {
        use tflip_storage::dd::{Dictionary, Kind};
        let session = Session::new();
        let fx = ops_fixtures();
        let check = |dir: &Path, prefix: &str| {
            for (file, golden) in [
                ("db.add", format!("{prefix}.add")),
                ("db.am", format!("{prefix}.am")),
                ("db.ai", format!("{prefix}.ai")),
            ] {
                let ours = std::fs::read(dir.join(file)).unwrap();
                let want = std::fs::read(fx.join(&golden)).unwrap();
                let (ours, want) = if file == "db.add" {
                    (mask_user_secrets(ours), mask_user_secrets(want))
                } else {
                    (ours, want)
                };
                assert_eq!(ours, want, "{golden}");
            }
        };

        // Add: dict + two groups + a user, then add the user to each group.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddmemadd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_group_api(&session, connect, "G1", "").unwrap();
        create_group_api(&session, connect, "G2", "").unwrap();
        create_user_api(&session, connect, "", "bob", "p", "").unwrap();
        add_user_to_group_api(&session, connect, "G1", "bob").unwrap();
        add_user_to_group_api(&session, connect, "G2", "bob").unwrap();
        check(&dir, "memadd");
        // bob is now in G1 and G2, decrypting to their ids in that order.
        {
            let dict = Dictionary::open(&add).unwrap();
            let key = dict.dictionary_key(b"").unwrap();
            let g1 = dict.named(Kind::Group, "G1").unwrap().id;
            let g2 = dict.named(Kind::Group, "G2").unwrap().id;
            let bob = dict.named(Kind::User, "bob").unwrap();
            assert_eq!(decode_membership(dict.scheme(), bob, &key), vec![g1, g2]);
            assert_eq!(bob.info1, Some(2));
        }
        // Already a member, or a missing group/user, is refused.
        assert!(matches!(
            add_user_to_group_api(&session, connect, "G1", "bob"),
            Err(Error::Sql(_))
        ));
        assert!(matches!(
            add_user_to_group_api(&session, connect, "Nope", "bob"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);

        // Remove: the same, then remove the user from the first group.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddmemrem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_group_api(&session, connect, "G1", "").unwrap();
        create_group_api(&session, connect, "G2", "").unwrap();
        create_user_api(&session, connect, "", "bob", "p", "").unwrap();
        add_user_to_group_api(&session, connect, "G1", "bob").unwrap();
        add_user_to_group_api(&session, connect, "G2", "bob").unwrap();
        remove_user_from_group_api(&session, connect, "G1", "bob").unwrap();
        check(&dir, "memrem");
        {
            let dict = Dictionary::open(&add).unwrap();
            let key = dict.dictionary_key(b"").unwrap();
            let g2 = dict.named(Kind::Group, "G2").unwrap().id;
            let bob = dict.named(Kind::User, "bob").unwrap();
            assert_eq!(decode_membership(dict.scheme(), bob, &key), vec![g2]);
        }
        // Not a member of that group is refused.
        assert!(matches!(
            remove_user_from_group_api(&session, connect, "G1", "bob"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One Permission object, deciphered: grantee id, target id, then slots 1-4
    /// (object rights, per-column rights, with-grant object rights, with-grant
    /// per-column rights), `None` for an unset slot.
    type PermissionPlain = (
        u32,
        i32,
        Option<u32>,
        Option<Vec<u8>>,
        Option<u32>,
        Option<Vec<u8>>,
    );

    /// Every Permission object of the dictionary at `add`, deciphered with its own
    /// key. Comparing these between tflip's file and a golden checks the
    /// enciphered slots `mask_user_secrets` blanks (each dictionary has its own
    /// key, so only the plaintext can match).
    fn permission_plaintext(add: &Path) -> Vec<PermissionPlain> {
        use tflip_storage::crypto;
        use tflip_storage::dd::{Dictionary, Kind, Object, permission};
        let dict = Dictionary::open(add).unwrap();
        let key = dict.dictionary_key(b"").unwrap();
        let plain = |p: &Object, slot: usize| {
            p.slot(slot).map(|raw| {
                let mut blob = raw.to_vec();
                crypto::crypt(
                    dict.scheme(),
                    &key,
                    crypto::permission_item(p.id, slot as u32),
                    &mut blob,
                );
                blob
            })
        };
        let rights =
            |p: &Object, slot| plain(p, slot).map(|b| crypto::permission_rights(&b).unwrap());
        let columns = |p: &Object, slot| {
            plain(p, slot).map(|b| crypto::permission_column_rights(&b).unwrap())
        };
        dict.all(Kind::Permission)
            .into_iter()
            .map(|p| {
                (
                    p.parent,
                    p.info1.unwrap(),
                    rights(p, permission::OBJECT_RIGHTS),
                    columns(p, permission::COLUMN_RIGHTS),
                    rights(p, permission::WITH_GRANT_RIGHTS),
                    columns(p, permission::WITH_GRANT_COLUMN_RIGHTS),
                )
            })
            .collect()
    }

    /// Replays the original engine's permission calls recorded by
    /// `tools/ace/dd/pubperm.c` (`fixtures/dictionary/pubperm.json`): grants,
    /// revokes and reads on publications, articles, subscriptions and the other
    /// kinds, with every right, the grant option, the inherit bit and the error
    /// cases. Each call's return code (and a read's mask) must match the engine's;
    /// then each scenario's dictionary must match its golden byte for byte (the
    /// per-dictionary-key ciphertext masked) and decipher to the same rights.
    #[test]
    fn replays_the_engines_permission_calls() {
        use std::collections::HashMap;
        use tflip_storage::dd::Dictionary;
        let session = Session::new();
        let fx = ops_fixtures();
        let text = std::fs::read_to_string(fx.join("../pubperm.json")).unwrap();
        let calls: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        let root = tflip_storage::testing::temp_root()
            .join(format!("tflip-pubperm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // The probe's base dictionary: bob, G1, pub and sub, and for some
        // scenarios table t with an article on it.
        let with_table = [
            "bits", "inh", "valid", "revkinds", "revnone", "colrev", "colrev0",
        ];
        let make = |name: &str| -> (std::path::PathBuf, Handle) {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let add = dir.join("db.add");
            let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
            create_user_api(&session, connect, "", "bob", "pw", "").unwrap();
            create_group_api(&session, connect, "G1", "").unwrap();
            create_publication_api(&session, connect, "pub", "", 0).unwrap();
            create_subscription_api(
                &session,
                connect,
                "sub",
                "pub",
                "\\\\srv\\t.add",
                "u",
                "pw",
                "q.add",
                0,
                "",
                0,
            )
            .unwrap();
            if with_table.contains(&name) {
                copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
                std::fs::copy(fx.join("t.adi"), dir.join("t.adi")).unwrap();
                add_table_api(
                    &session,
                    connect,
                    "t",
                    "t.adt",
                    TableType::Adt,
                    1,
                    "t.adi",
                    "",
                )
                .unwrap();
                create_article_api(&session, connect, "pub", "t", "id", "", 0, false).unwrap();
            }
            (dir, connect)
        };
        let code = |e: Error| match e {
            Error::Sql(e) => e.native,
            Error::InvalidObjectName => 5132,
            Error::InvalidObjectType => 5136,
            other => panic!("unexpected error {other:?}"),
        };
        // Object id -> name, to match Permission objects across dictionaries
        // whose object sets differ.
        let names = |add: &Path| -> HashMap<u32, String> {
            let dict = Dictionary::open(add).unwrap();
            dict.objects()
                .iter()
                .map(|o| (o.id, o.name.clone()))
                .collect()
        };
        let compared = std::cell::Cell::new(0);
        let finish = |name: &str, dir: &Path, connect: Handle| {
            connection::disconnect(&session, connect).ok();
            let ours = dir.join("db.add");
            let golden = fx.join(format!("{name}.add"));
            if !golden.exists() {
                return;
            }
            compared.set(compared.get() + 1);
            let (ours_plain, want_plain) =
                (permission_plaintext(&ours), permission_plaintext(&golden));
            if name == "revkinds" {
                // The golden also has a view, procedure, function, package and
                // link tflip's base lacks: compare the grants on the objects both
                // have.
                let (ours_names, want_names) = (names(&ours), names(&golden));
                let shared: Vec<_> = want_plain
                    .into_iter()
                    .filter(|p| ours_names.get(&(p.1 as u32)) == want_names.get(&(p.1 as u32)))
                    .collect();
                assert_eq!(ours_plain, shared, "{name} permissions");
                return;
            }
            assert_eq!(ours_plain, want_plain, "{name} permissions");
            for ext in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("db.{ext}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{name}.{ext}"))).unwrap();
                let (ours, want) = match ext {
                    "add" => (mask_user_secrets(ours), mask_user_secrets(want)),
                    _ => (ours, want),
                };
                assert_eq!(ours, want, "{name}.{ext}");
            }
        };

        let mut current: Option<(String, std::path::PathBuf, Handle)> = None;
        let (mut replayed, mut dumps) = (0, 0);
        for call in &calls {
            let scenario = call["scenario"].as_str().unwrap();
            // "order" places publications among kinds tflip's base does not make.
            if scenario == "order" {
                continue;
            }
            if current.as_ref().is_none_or(|(name, ..)| name != scenario) {
                if let Some((name, dir, connect)) = current.take() {
                    finish(&name, &dir, connect);
                }
                let (dir, connect) = make(scenario);
                current = Some((scenario.to_owned(), dir, connect));
            }
            let connect = current.as_ref().unwrap().2;
            let op = call["op"].as_str().unwrap();
            let want_rc = call["rc"].as_u64().unwrap() as u32;
            if op == "AddUserToGroup G1 bob" {
                add_user_to_group_api(&session, connect, "G1", "bob").unwrap();
                continue;
            }
            if let Some(rows) = call.get("rows") {
                // A system.permissions / system.effectivepermissions dump,
                // narrowed as the probe's WHERE narrows it.
                use tflip_storage::value::Value;
                let table = if op.contains("effective") {
                    "effectivepermissions"
                } else {
                    "permissions"
                };
                let built = connection::get(&session, connect)
                    .unwrap()
                    .with_database(|db| Ok(crate::system::build(Some(db), table).unwrap().unwrap()))
                    .unwrap();
                let bob_only = op.contains("Grantee = 'bob'");
                let ours: Vec<serde_json::Value> = built
                    .rows
                    .iter()
                    .filter(|row| match bob_only {
                        true => row[3] == Value::Text("bob".into()),
                        false => matches!(row[1], Value::Integer(15 | 17)),
                    })
                    .map(|row| {
                        row.iter()
                            .map(|v| match v {
                                Value::Text(s) => serde_json::json!(s.trim_end()),
                                Value::Integer(i) => serde_json::json!(i.to_string()),
                                Value::Null => serde_json::Value::Null,
                                other => panic!("unexpected {other:?}"),
                            })
                            .collect()
                    })
                    .collect();
                assert_eq!(&serde_json::Value::Array(ours), rows, "{scenario}: {op}");
                dumps += 1;
                continue;
            }
            let mut words = op.split(' ');
            let verb = words.next().unwrap();
            // SQL GRANT/REVOKE: the return code and, for a failure, the message.
            if verb == "GRANT" || verb == "REVOKE" {
                let handle = crate::statement::create(&session, connect).unwrap();
                let done = crate::statement::execute_direct(&session, handle, op, false);
                crate::statement::close(&session, handle).ok();
                match done {
                    Ok(_) => assert_eq!(want_rc, 0, "{scenario}: {op}"),
                    Err(Error::Sql(e)) => {
                        assert_eq!(want_rc, 7200, "{scenario}: {op}");
                        let want = call["error"].as_str().unwrap();
                        assert!(
                            want.ends_with(&e.message),
                            "{scenario}: {op}: {}",
                            e.message
                        );
                    }
                    Err(other) => panic!("{scenario}: {op}: {other:?}"),
                }
                replayed += 1;
                continue;
            }
            // The SQL that builds a scenario's other objects.
            if !matches!(verb, "grant" | "revoke" | "get") {
                continue;
            }
            let args: HashMap<&str, &str> = words
                .filter_map(|w| w.split_once('='))
                .map(|(k, v)| (k, if v == "(null)" { "" } else { v }))
                .collect();
            let object_type: u16 = args["type"].parse().unwrap();
            let name = args["name"];
            // The kinds tflip's base does not make.
            if ["v", "p", "f", "pk", "lnk"].contains(&name) {
                continue;
            }
            let (parent, grantee) = (args["parent"], args["grantee"]);
            replayed += 1;
            let got = if verb == "get" {
                let inherited = args["inherited"] == "1";
                get_permissions_api(
                    &session,
                    connect,
                    grantee,
                    object_type,
                    name,
                    parent,
                    inherited,
                )
                .map(Some)
            } else {
                let bits = u32::from_str_radix(args["bits"].trim_start_matches("0x"), 16).unwrap();
                let change = if verb == "grant" {
                    grant_permission_api
                } else {
                    revoke_permission_api
                };
                change(&session, connect, object_type, name, parent, grantee, bits).map(|()| None)
            };
            match got {
                Ok(mask) => {
                    assert_eq!(want_rc, 0, "{scenario}: {op}");
                    if let Some(mask) = mask {
                        assert_eq!(format!("0x{mask:x}"), call["mask"], "{scenario}: {op}");
                    }
                }
                Err(e) => assert_eq!(code(e), want_rc, "{scenario}: {op}"),
            }
        }
        if let Some((name, dir, connect)) = current.take() {
            finish(&name, &dir, connect);
        }
        // Every recorded call but the skipped kinds, and all 15 goldens.
        assert_eq!(replayed, 388);
        assert_eq!(dumps, 4);
        assert_eq!(compared.get(), 15);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every user's and group's rights to create (slot 6), deciphered with the
    /// dictionary's own key, by name.
    fn create_rights_plaintext(add: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        use tflip_storage::crypto;
        use tflip_storage::dd::{Dictionary, Kind, permission};
        let dict = Dictionary::open(add).unwrap();
        let key = dict.dictionary_key(b"").unwrap();
        let mut grantees = dict.all(Kind::User);
        grantees.extend(dict.all(Kind::Group));
        grantees
            .into_iter()
            .map(|o| {
                let bytes = o.slot(permission::CREATE_RIGHTS).map(|raw| {
                    let mut b = raw.to_vec();
                    let item = crypto::permission_item(o.id, permission::CREATE_RIGHTS as u32);
                    crypto::crypt(dict.scheme(), &key, item, &mut b);
                    crypto::permission_column_rights(&b).unwrap()
                });
                (o.name.clone(), bytes)
            })
            .collect()
    }

    /// A slot of `DB:Public`'s rights that does not check is 5127 to whoever reads
    /// it for the grantee (`fixtures/dictionary/public`'s probe: any byte changed,
    /// the checksum or the zeros between, is refused alike), and to a grant or
    /// revoke on the object; a user's own rights, not through the public ones, are
    /// read as ever.
    #[test]
    fn refuses_a_public_rights_slot_that_does_not_check() {
        use tflip_storage::dd::{Dictionary, Kind};
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-pubbad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        create_user_api(&session, connect, "", "bob", "pw", "").unwrap();
        grant_permission_api(&session, connect, 1, "t", "", "DB:Public", 0x1).unwrap();
        connection::disconnect(&session, connect).ok();

        let raw = Dictionary::open(&add)
            .unwrap()
            .named(Kind::Table, "t")
            .unwrap()
            .slot(12)
            .unwrap()
            .to_vec();
        for byte in [0, 8, 16, 31] {
            let mut bytes = std::fs::read(&add).unwrap();
            let at = bytes.windows(32).position(|w| w == raw).unwrap();
            bytes[at + byte] ^= 1;
            std::fs::write(&add, &bytes).unwrap();
            let connect =
                connection::connect_path(&session, &add.to_string_lossy(), 1, Some("ADSSYS"), None)
                    .unwrap();
            let get = |grantee: &str, inherited| {
                get_permissions_api(&session, connect, grantee, 1, "t", "", inherited)
            };
            let refused =
                |result: Result<_, Error>| matches!(result, Err(Error::Sql(e)) if e.native == 5127);
            assert!(refused(get("DB:Public", false)), "byte {byte}");
            assert!(refused(get("bob", true)), "byte {byte}");
            assert_eq!(get("bob", false).unwrap(), 0x8, "byte {byte}");
            assert!(refused(
                grant_permission_api(&session, connect, 1, "t", "", "DB:Public", 0x2).map(|_| 0)
            ));
            assert!(refused(
                revoke_permission_api(&session, connect, 1, "t", "", "DB:Public", 0x1).map(|_| 0)
            ));
            connection::disconnect(&session, connect).ok();
            bytes[at + byte] ^= 1;
            std::fs::write(&add, &bytes).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What each object holds for `DB:Public` in its own slot: its name and rights.
    fn public_rights_plaintext(add: &Path) -> Vec<(String, tflip_storage::dd::PublicRights)> {
        use tflip_storage::dd::{self, Dictionary};
        let dict = Dictionary::open(add).unwrap();
        let key = dict.dictionary_key(b"").unwrap();
        dict.objects()
            .iter()
            .filter_map(|o| match dict.public_rights(o, Some(&key)) {
                dd::PublicRights::Absent => None,
                rights => Some((o.name.clone(), rights)),
            })
            .collect()
    }

    /// Replays the original engine's SQL `GRANT`/`REVOKE` statements and
    /// permission calls recorded by `tools/ace/dd/grants.c` from
    /// `tools/ace/dd/grants.txt` (`fixtures/dictionary/grants.json`): object type
    /// 0, the rights to create, the per-column rights, the inherit bit, built-in
    /// groups and the engine's errors. Each step's return code — and error text, a
    /// read's mask, a `system.permissions` dump — must match the engine's; then
    /// each scenario's dictionary must match its golden byte for byte (the
    /// per-dictionary-key ciphertext masked) and decipher to the same rights.
    #[test]
    fn replays_the_engines_grants() {
        replay_grants("grants.json", "grants_", 10, (300, 6));
    }

    /// Replays `fixtures/dictionary/public.json`, recorded by `tools/ace/dd/public.sh`
    /// from `tools/ace/dd/public.txt`: the rights granted to `DB:Public`, which
    /// the engine keeps in the object itself, and what they give a user, as
    /// [`replays_the_engines_grants`] replays the other grantees'. The slots'
    /// random parts are masked in the comparison and their plaintext compared.
    #[test]
    fn replays_the_engines_public_rights() {
        replay_grants("public.json", "", 3, (100, 5));
    }

    /// The steps of the recording `file` of `fixtures/dictionary`, each
    /// scenario's dictionary compared with the golden `<golden><scenario>`
    /// (`goldens` of them); at least `checked_wanted` of the steps are checked and
    /// `dumps_wanted` of them dump a system table.
    fn replay_grants(
        file: &str,
        golden: &str,
        goldens: usize,
        (checked_wanted, dumps_wanted): (usize, usize),
    ) {
        use crate::statement;
        use tflip_storage::value::Value;
        let session = Session::new();
        let fx = ops_fixtures();
        let text = std::fs::read_to_string(fx.join("..").join(file)).unwrap();
        let steps: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        let root = tflip_storage::testing::temp_root()
            .join(format!("tflip-grants-{file}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // An error as the engine reports it: the code and, after `Error N:  `,
        // the text — for SQL the SQL state, native code and message.
        let error_of = |e: Error| -> SqlError {
            match e {
                Error::Sql(e) => e,
                other => panic!("unexpected error {other:?}"),
            }
        };
        let compared = std::cell::Cell::new(0);
        let finish = |name: &str, dir: &Path, connect: Handle| {
            connection::disconnect(&session, connect).ok();
            let golden_add = fx.join(format!("{golden}{name}.add"));
            if !golden_add.exists() {
                return;
            }
            compared.set(compared.get() + 1);
            let ours = dir.join("db.add");
            assert_eq!(
                permission_plaintext(&ours),
                permission_plaintext(&golden_add),
                "{name}"
            );
            assert_eq!(
                create_rights_plaintext(&ours),
                create_rights_plaintext(&golden_add),
                "{name} create rights"
            );
            assert_eq!(
                public_rights_plaintext(&ours),
                public_rights_plaintext(&golden_add),
                "{name} rights of DB:Public"
            );
            for ext in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("db.{ext}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{golden}{name}.{ext}"))).unwrap();
                let (ours, want) = match ext {
                    "add" => (mask_user_secrets(ours), mask_user_secrets(want)),
                    _ => (ours, want),
                };
                let first = ours.iter().zip(&want).position(|(a, b)| a != b);
                assert!(
                    ours.len() == want.len() && first.is_none(),
                    "{golden}{name}.{ext}: lengths {} and {}, first difference at {first:?}",
                    ours.len(),
                    want.len()
                );
            }
        };

        let mut current: Option<(String, std::path::PathBuf, Handle)> = None;
        let (mut checked, mut dumps) = (0, 0);
        for step in &steps {
            let scenario = step["scenario"].as_str().unwrap();
            if current.as_ref().is_none_or(|(name, ..)| name != scenario) {
                if let Some((name, dir, connect)) = current.take() {
                    finish(&name, &dir, connect);
                }
                let dir = root.join(scenario);
                std::fs::create_dir_all(&dir).unwrap();
                let add = dir.join("db.add");
                let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
                current = Some((scenario.to_owned(), dir, connect));
            }
            let (_, dir, connect) = current.as_ref().unwrap();
            let connect = *connect;
            let op = step["op"].as_str().unwrap();
            let want_rc = step["rc"].as_u64().unwrap() as u32;
            let (verb, rest) = op.split_once(' ').unwrap_or((op, ""));

            // The probe goes on as another user, or as the administrator.
            if matches!(verb, "as" | "asadmin") {
                connection::disconnect(&session, connect).ok();
                let add = dir.join("db.add").to_string_lossy().into_owned();
                let (user, password) = match verb {
                    "as" => (rest, Some("pw")),
                    _ => ("ADSSYS", None),
                };
                let handle = connection::connect_path(&session, &add, 1, Some(user), password);
                assert_eq!(handle.is_ok(), want_rc == 0, "{scenario}: {op}");
                current.as_mut().unwrap().2 = handle.unwrap();
                continue;
            }

            // A `system.permissions` dump, narrowed as the probe's WHERE narrows it.
            if let Some(rows) = step.get("rows") {
                let table = if op.contains("effective") {
                    "effectivepermissions"
                } else {
                    "permissions"
                };
                let built = connection::get(&session, connect)
                    .unwrap()
                    .with_database(|db| Ok(crate::system::build(Some(db), table).unwrap().unwrap()))
                    .unwrap();
                let quoted: Vec<&str> = op.split('\'').skip(1).step_by(2).collect();
                let text_of = |v: &Value| match v {
                    Value::Text(s) => s.trim_end().to_owned(),
                    _ => String::new(),
                };
                let int_of = |v: &Value| match v {
                    Value::Integer(i) => *i as i64,
                    _ => -1,
                };
                let ours: Vec<serde_json::Value> = built
                    .rows
                    .iter()
                    .filter(|row| quoted.contains(&text_of(&row[3]).as_str()))
                    .filter(|row| !op.contains("\"Create\" IS NOT NULL") || row[11] != Value::Null)
                    .filter(|row| !op.contains("Object_Type <> 4") || int_of(&row[1]) != 4)
                    .filter(|row| {
                        !op.contains("Object_Type IN (1, 4)") || matches!(int_of(&row[1]), 1 | 4)
                    })
                    .map(|row| {
                        row.iter()
                            .map(|v| match v {
                                Value::Text(s) => serde_json::json!(s.trim_end()),
                                Value::Integer(i) => serde_json::json!(i.to_string()),
                                Value::Null => serde_json::Value::Null,
                                other => panic!("unexpected {other:?}"),
                            })
                            .collect()
                    })
                    .collect();
                assert_eq!(&serde_json::Value::Array(ours), rows, "{scenario}: {op}");
                dumps += 1;
                continue;
            }

            let result: Result<Option<u32>, Error> = if op.contains('|') {
                // An API call: grant/revoke T|NAME|PARENT|GRANTEE|BITS or get
                // GRANTEE|T|NAME|PARENT|INHERITED; `-` is a null name.
                let f: Vec<&str> = rest.split('|').collect();
                fn name(s: &str) -> &str {
                    if s == "-" { "" } else { s }
                }
                let null = |s: &str| s == "-";
                if verb == "get" {
                    get_permissions_api(
                        &session,
                        connect,
                        f[0],
                        f[1].parse().unwrap(),
                        name(f[2]),
                        name(f[3]),
                        f[4] == "1",
                    )
                    .map(Some)
                } else {
                    let object_type: u16 = f[0].parse().unwrap();
                    let bits = u32::from_str_radix(f[4].trim_start_matches("0x"), 16).unwrap();
                    let base = bits & !0x8000_0000;
                    let all = bits == 0xFFFF_FFFF || bits == 0x8FFF_FFFF;
                    // The DLL refuses a null name where one is needed before the
                    // engine sees it (`tflip-ace`'s `change_permission`).
                    let named = matches!(
                        object_type,
                        0 | 1 | 4 | 6 | 8 | 9 | 10 | 12 | 15 | 17 | 18 | 19
                    );
                    if null(f[1]) && named && base != 0 && base & 0x80 == 0 && !all {
                        Err(permissions::empty_object_name())
                    } else {
                        let change = if verb == "grant" {
                            grant_permission_api
                        } else {
                            revoke_permission_api
                        };
                        change(
                            &session,
                            connect,
                            object_type,
                            name(f[1]),
                            name(f[2]),
                            f[3],
                            bits,
                        )
                        .map(|()| None)
                    }
                }
            } else {
                match verb {
                    "user" => create_user_api(&session, connect, "", rest, "pw", ""),
                    "group" => create_group_api(&session, connect, rest, ""),
                    "member" => {
                        let (group, user) = rest.split_once(' ').unwrap();
                        add_user_to_group_api(&session, connect, group, user)
                    }
                    "table" => {
                        let file = format!("{rest}.adt");
                        copy_free(&fx.join("t.adt"), &dir.join(&file));
                        add_table_api(&session, connect, rest, &file, TableType::Adt, 1, "", "")
                    }
                    "view" => {
                        let (name, sql) = rest.split_once(' ').unwrap();
                        add_view_api(&session, connect, name, sql, "")
                    }
                    "proc" => add_procedure_api(
                        &session, connect, rest, ".\\x.dll", "Proc", 1, "a,I;", "b,I;", "",
                    ),
                    "link" => add_link_api(&session, connect, rest, "db.add", "bob", "pw", 1),
                    "pub" => create_publication_api(&session, connect, rest, "", 0),
                    "sub" => create_subscription_api(
                        &session,
                        connect,
                        rest,
                        "pb",
                        "\\\\srv\\t.add",
                        "u",
                        "pw",
                        "q.add",
                        0,
                        "",
                        0,
                    ),
                    "verify" => {
                        let on: u16 = rest.parse().unwrap();
                        set_database_property_api(&session, connect, 104, &on.to_le_bytes())
                    }
                    // SQL.
                    _ => {
                        let handle = statement::create(&session, connect).unwrap();
                        let done = statement::execute_direct(&session, handle, op, false);
                        statement::close(&session, handle).ok();
                        done.map(|_| ())
                    }
                }
                .map(|()| None)
                .or_else(|e| match e {
                    Error::NoCursorHandle => Ok(None),
                    e => Err(e),
                })
            };
            checked += 1;
            match result {
                Ok(mask) => {
                    assert_eq!(want_rc, 0, "{scenario}: {op}");
                    if let Some(mask) = mask {
                        assert_eq!(format!("0x{mask:x}"), step["mask"], "{scenario}: {op}");
                    }
                }
                Err(Error::InvalidObjectName) if verb == "get" => {
                    assert_eq!(want_rc, 5132, "{scenario}: {op}")
                }
                Err(Error::InvalidObjectType) if verb == "get" => {
                    assert_eq!(want_rc, 5136, "{scenario}: {op}")
                }
                Err(e) => {
                    let e = error_of(e);
                    let want = step["error"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{scenario}: {op}: refused with {e:?}"));
                    if want_rc == 7200 {
                        // "Error 7200:  AQE Error:  State = S;   NativeError = N;
                        // [SAP][Advantage SQL Engine]<message>"
                        let (_, tail) = want.split_once("State = ").unwrap();
                        let (state, tail) = tail.split_once(";   NativeError = ").unwrap();
                        let (code, tail) = tail.split_once(";  ").unwrap();
                        let message = tail.strip_prefix("[SAP][Advantage SQL Engine]").unwrap();
                        let got = (e.state.as_str(), e.native.to_string(), e.message.as_str());
                        assert_eq!(got, (state, code.to_owned(), message), "{scenario}: {op}");
                    } else {
                        assert_eq!(e.native, want_rc, "{scenario}: {op}");
                        assert_eq!(format!("[ASA] {want}"), e.message, "{scenario}: {op}");
                    }
                }
            }
        }
        if let Some((name, dir, connect)) = current.take() {
            finish(&name, &dir, connect);
        }
        assert_eq!(compared.get(), goldens);
        assert!(
            checked > checked_wanted && dumps == dumps_wanted,
            "{checked} steps, {dumps} dumps"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn grants_and_revokes_permissions_like_the_engine() {
        use tflip_storage::crypto;
        use tflip_storage::dd::{Dictionary, Kind, permission};
        let session = Session::new();
        let fx = ops_fixtures();

        let decoded = |add: &Path, grantee_oid: u32, target_oid: u32| -> u32 {
            let dict = Dictionary::open(add).unwrap();
            let key = dict.dictionary_key(b"").unwrap();
            let perm = dict
                .all(Kind::Permission)
                .into_iter()
                .find(|p| p.parent == grantee_oid && p.info1 == Some(target_oid as i32))
                .unwrap();
            let mut blob = perm.slot(permission::OBJECT_RIGHTS).unwrap().to_vec();
            crypto::crypt(
                dict.scheme(),
                &key,
                crypto::permission_item(perm.id, permission::OBJECT_RIGHTS as u32),
                &mut blob,
            );
            crypto::permission_rights(&blob).unwrap()
        };
        let base = |dir: &Path| -> Handle {
            let add = dir.join("db.add");
            let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
            copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
            add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
            create_user_api(&session, connect, "", "bob", "pw", "").unwrap();
            create_group_api(&session, connect, "G1", "").unwrap();
            connect
        };
        let check = |dir: &Path, prefix: &str| {
            for part in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("db.{part}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{prefix}.{part}"))).unwrap();
                let (ours, want) = if part == "add" {
                    (mask_user_secrets(ours), mask_user_secrets(want))
                } else {
                    (ours, want)
                };
                assert_eq!(ours, want, "{prefix}.{part}");
            }
        };

        // Grant: READ then INSERT to bob (accumulate into one object), READ to G1.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddperm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = base(&dir);
        grant_permission_api(&session, connect, 1, "t", "", "bob", 0x1).unwrap();
        grant_permission_api(&session, connect, 1, "t", "", "bob", 0x10).unwrap();
        grant_permission_api(&session, connect, 1, "t", "", "G1", 0x1).unwrap();
        check(&dir, "permgrant");
        // bob (user id 5) on table t (id 2): READ|INSERT|INHERIT; G1 (id 6): READ.
        assert_eq!(decoded(&add, 5, 2), 0x1 | 0x10 | permission::INHERIT);
        assert_eq!(decoded(&add, 6, 2), 0x1);
        // Invalid right bits are refused; an unknown grantee too.
        assert!(matches!(
            grant_permission_api(&session, connect, 1, "t", "", "bob", 0x4),
            Err(Error::Sql(e)) if e.native == 5158
        ));
        assert!(matches!(
            grant_permission_api(&session, connect, 1, "t", "", "ghost", 0x1),
            Err(Error::Sql(_))
        ));
        // An object type that has no permissions (a relation) is refused.
        assert!(matches!(
            grant_permission_api(&session, connect, 2, "t", "", "bob", 0x1),
            Err(Error::Sql(e)) if e.native == 5158
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);

        // Revoke: grant READ|UPDATE to bob, then revoke UPDATE.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermrev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = base(&dir);
        grant_permission_api(&session, connect, 1, "t", "", "bob", 0x3).unwrap();
        revoke_permission_api(&session, connect, 1, "t", "", "bob", 0x2).unwrap();
        check(&dir, "permrevoke");
        // bob keeps READ (and the inherit bit); UPDATE is gone.
        assert_eq!(decoded(&add, 5, 2), 0x1 | permission::INHERIT);
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);

        // Decodes a per-column rights slot of bob's Permission on table t.
        let columns = |add: &Path, slot: usize| -> Vec<u8> {
            let dict = Dictionary::open(add).unwrap();
            let key = dict.dictionary_key(b"").unwrap();
            let perm = dict
                .all(Kind::Permission)
                .into_iter()
                .find(|p| p.parent == 5 && p.info1 == Some(2))
                .unwrap();
            let mut blob = perm.slot(slot).unwrap().to_vec();
            crypto::crypt(
                dict.scheme(),
                &key,
                crypto::permission_item(perm.id, slot as u32),
                &mut blob,
            );
            crypto::permission_column_rights(&blob).unwrap()
        };
        let one = |dir: &Path, f: &dyn Fn(Handle)| {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).unwrap();
            let connect = base(dir);
            f(connect);
            connection::disconnect(&session, connect).ok();
        };

        // Column grant: READ on column s, then UPDATE on column id. Each column
        // keeps its own byte in slot 2; slot 1 aggregates their bits.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermcol-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 4, "s", "t", "bob", 0x1).unwrap();
            grant_permission_api(&session, connect, 4, "id", "t", "bob", 0x2).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permcol");
        assert_eq!(columns(&add, permission::COLUMN_RIGHTS), vec![0x02, 0x01]); // id:UPDATE, s:READ
        assert_eq!(decoded(&add, 5, 2), 0x1 | 0x2 | permission::INHERIT);
        let _ = std::fs::remove_dir_all(&dir);

        // Grant option on a table: READ normally, then INSERT with the grant
        // option. Slot 1 holds both; slot 3 only the with-grant bit.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermgopt-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 1, "t", "", "bob", 0x1).unwrap();
            grant_permission_api(&session, connect, 1, "t", "", "bob", 0x10 | 0x8000_0000).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permgopt");
        assert_eq!(decoded(&add, 5, 2), 0x1 | 0x10 | permission::INHERIT);
        let _ = std::fs::remove_dir_all(&dir);

        // Column revoke: grant READ|UPDATE on column s, revoke UPDATE. Slot 2's
        // byte loses UPDATE and slot 1 follows; slot 3 is written zero.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermcolrev-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 4, "s", "t", "bob", 0x3).unwrap();
            revoke_permission_api(&session, connect, 4, "s", "t", "bob", 0x2).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permcolrev");
        assert_eq!(columns(&add, permission::COLUMN_RIGHTS), vec![0x00, 0x01]); // s keeps READ
        let _ = std::fs::remove_dir_all(&dir);

        // Column grant option: READ with the grant option on column s writes the
        // with-grant column slot 4 (and slot 3) alongside the regular ones.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermcolg-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 4, "s", "t", "bob", 0x1 | 0x8000_0000).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permcolgopt");
        assert_eq!(
            columns(&add, permission::WITH_GRANT_COLUMN_RIGHTS),
            vec![0x00, 0x01]
        );
        let _ = std::fs::remove_dir_all(&dir);

        // Grant ALTER on the user bob (object type 8) to the group G1: a group
        // grantee carries no inherit bit.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermuser-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 8, "bob", "", "G1", 0x100).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permuser");
        assert_eq!(decoded(&add, 6, 5), 0x100); // G1 (id 6) on user bob (id 5): ALTER
        let _ = std::fs::remove_dir_all(&dir);

        // Grant ALTER on the group G1 (object type 9) to the user bob.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermgroup-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 9, "G1", "", "bob", 0x100).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permgroup");
        assert_eq!(decoded(&add, 5, 6), 0x100 | permission::INHERIT);
        let _ = std::fs::remove_dir_all(&dir);

        // Grant ALTER on the database itself (object type 11, no object name) to
        // the user bob; the target is the Database object (id 1).
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpermdb-{}", std::process::id()));
        one(&dir, &|connect| {
            grant_permission_api(&session, connect, 11, "", "", "bob", 0x100).unwrap();
        });
        let add = dir.join("db.add");
        check(&dir, "permdb");
        assert_eq!(decoded(&add, 5, 1), 0x100 | permission::INHERIT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_back_group_membership_it_wrote() {
        use crate::dictionary::{Database, ObjectRef, Property};
        use tflip_storage::dd::Kind;
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddmemread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_group_api(&session, connect, "Staff", "").unwrap();
        create_group_api(&session, connect, "Admins", "").unwrap();
        create_user_api(&session, connect, "Staff", "bob", "pw", "").unwrap();
        create_user_api(&session, connect, "", "carol", "", "").unwrap();
        add_user_to_group_api(&session, connect, "Admins", "bob").unwrap();
        connection::disconnect(&session, connect).ok();

        let database = Database::open(&add).unwrap();
        // The decrypted member groups, in membership order.
        let bob = database.dictionary().named(Kind::User, "bob").unwrap();
        assert_eq!(
            database.user_groups(bob),
            Some(vec!["Staff".to_owned(), "Admins".to_owned()])
        );
        let carol = database.dictionary().named(Kind::User, "carol").unwrap();
        assert_eq!(database.user_groups(carol), Some(Vec::new()));

        // Property 1102 lists them with DB:Public, as the engine returns it.
        let membership = |user: &str| match database
            .property(&ObjectRef::User(user.to_owned()), 1102)
            .unwrap()
        {
            Property::Text(text) => text,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(membership("bob"), "Staff;Admins;DB:Public");
        assert_eq!(membership("carol"), "DB:Public");

        // system.usergroupmembers lists each membership, the real groups then
        // DB:Public, one row per (user, group).
        let members = crate::system::build(Some(&database), "usergroupmembers")
            .unwrap()
            .unwrap();
        let rows: Vec<(String, String)> = members
            .rows
            .iter()
            .map(|row| {
                let cell = |v: &tflip_storage::value::Value| match v {
                    tflip_storage::value::Value::Text(t) => t.clone(),
                    other => panic!("unexpected {other:?}"),
                };
                (cell(&row[0]), cell(&row[1]))
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("bob".to_owned(), "Staff".to_owned()),
                ("bob".to_owned(), "Admins".to_owned()),
                ("bob".to_owned(), "DB:Public".to_owned()),
                ("carol".to_owned(), "DB:Public".to_owned()),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_a_user_in_a_group_like_the_engine() {
        use tflip_storage::dd::{Dictionary, Kind};
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddusergroup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_group_api(&session, connect, "Staff", "the staff").unwrap();
        create_user_api(&session, connect, "Staff", "bob", "pw", "Bob").unwrap();
        for (file, golden) in [
            ("db.add", "usergroup.add"),
            ("db.am", "usergroup.am"),
            ("db.ai", "usergroup.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_user_secrets(ours), mask_user_secrets(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "{file}");
        }
        // bob's membership slot decrypts, with the dictionary key, to the Staff
        // group's object id — as the engine stores it.
        let dict = Dictionary::open(&add).unwrap();
        let key = dict.dictionary_key(b"").unwrap();
        let staff_id = dict.named(Kind::Group, "Staff").unwrap().id;
        let bob = dict.named(Kind::User, "bob").unwrap();
        let mut membership = bob
            .slot(tflip_storage::dd::user::GROUP_MEMBERSHIP)
            .unwrap()
            .to_vec();
        tflip_storage::crypto::group_membership(dict.scheme(), &key, &mut membership);
        assert_eq!(membership, staff_id.to_le_bytes(), "bob is in Staff");
        assert_eq!(bob.info1, Some(1));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `AdsDDSetObjectAccessRights` parses its access string (`R`/`W`/`X`/`I`),
    /// infers the object's type from its name, validates the rights against it,
    /// and replaces the accessor's rights. The Permission object format is the
    /// grant path's (verified byte for byte elsewhere), so this checks the
    /// decoded mask by round-trip, as the PLAN calls for.
    #[test]
    fn sets_object_access_rights() {
        use tflip_storage::crypto;
        use tflip_storage::dd::{Dictionary, Kind, permission};
        let session = Session::new();
        let fx = ops_fixtures();
        let decoded = |add: &Path, grantee_oid: u32, target_oid: u32| -> Option<u32> {
            let dict = Dictionary::open(add).unwrap();
            let key = dict.dictionary_key(b"").unwrap();
            let perm = dict
                .all(Kind::Permission)
                .into_iter()
                .find(|p| p.parent == grantee_oid && p.info1 == Some(target_oid as i32))?;
            let mut blob = perm.slot(permission::OBJECT_RIGHTS).unwrap().to_vec();
            crypto::crypt(
                dict.scheme(),
                &key,
                crypto::permission_item(perm.id, permission::OBJECT_RIGHTS as u32),
                &mut blob,
            );
            crypto::permission_rights(&blob)
        };
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddsetobj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        add_procedure_api(&session, connect, "p", "lib.dll", "go", 1, "", "", "").unwrap();
        create_user_api(&session, connect, "", "bob", "pw", "").unwrap();

        // "W" on the table -> update|insert|delete; the table is id 2, bob id 3.
        set_object_access_rights_api(&session, connect, "t", "bob", "W").unwrap();
        let bob_oid = {
            let dict = Dictionary::open(&add).unwrap();
            dict.named(Kind::User, "bob").unwrap().id
        };
        let table_oid = {
            let dict = Dictionary::open(&add).unwrap();
            dict.named(Kind::Table, "t").unwrap().id
        };
        assert_eq!(
            decoded(&add, bob_oid, table_oid),
            Some(permission::UPDATE | permission::INSERT | permission::DELETE)
        );
        // "RI" replaces it with read|inherit (W's bits gone).
        set_object_access_rights_api(&session, connect, "t", "bob", "RI").unwrap();
        assert_eq!(
            decoded(&add, bob_oid, table_oid),
            Some(permission::READ | permission::INHERIT)
        );
        // The empty string clears the rights.
        set_object_access_rights_api(&session, connect, "t", "bob", "").unwrap();
        assert_eq!(decoded(&add, bob_oid, table_oid), Some(0));

        // Execute is invalid for a table (5158) but valid for a procedure.
        assert_eq!(
            set_object_access_rights_api(&session, connect, "t", "bob", "X"),
            Err(invalid_object_right(Kind::Table))
        );
        let Err(Error::Sql(refused)) =
            set_object_access_rights_api(&session, connect, "t", "bob", "X")
        else {
            panic!("a right a table has no use for is refused");
        };
        assert_eq!(
            refused.message,
            "[ASA] Error 5158:  The rights to be granted are not valid for the specified \
             object.  One of the specified permissions is not a valid table permission."
        );
        set_object_access_rights_api(&session, connect, "p", "bob", "X").unwrap();
        let proc_oid = {
            let dict = Dictionary::open(&add).unwrap();
            dict.named(Kind::Procedure, "p").unwrap().id
        };
        assert_eq!(decoded(&add, bob_oid, proc_oid), Some(permission::EXECUTE));
        // Read is invalid for a procedure.
        assert_eq!(
            set_object_access_rights_api(&session, connect, "p", "bob", "R"),
            Err(invalid_object_right(Kind::Procedure))
        );

        // An unknown object (5137) and an unknown accessor (5132).
        assert_eq!(
            set_object_access_rights_api(&session, connect, "nope", "bob", "R"),
            Err(no_object_found())
        );
        // The original's text (`tools/ace/dd/setobj.c`).
        let Err(Error::Sql(refused)) =
            set_object_access_rights_api(&session, connect, "nope", "bob", "R")
        else {
            panic!("an unknown object is refused");
        };
        assert_eq!(
            refused.message,
            "[ASA] Error 5137:  No object matching the search condition was found in the \
             data dictionary.  The given object was not found."
        );
        assert_eq!(
            set_object_access_rights_api(&session, connect, "t", "ghost", "R"),
            Err(invalid_object_name("ghost"))
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
