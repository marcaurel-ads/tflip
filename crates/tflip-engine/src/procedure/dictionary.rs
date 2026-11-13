//! The system procedures that change a dictionary's objects
//! (`EXECUTE PROCEDURE sp_CreateUser(...)` and the others), over the same
//! writes the `AdsDD*` calls make. What they check, and the texts of their
//! errors, are the SQL engine's own and differ from the calls' (`fixtures/sql/spdd*.json`).
//!
//! The SQL engine checks the arguments against the procedure's parameters
//! first (a type that does not fit, the number of arguments), runs it, and
//! only then complains of a string longer than its parameter: the object is
//! made with the value cut to the parameter's length.

mod props;

use tflip_sql::ast::Execute;
use tflip_sql::{SqlError, V};
use tflip_storage::dd::{self, Dictionary, Kind, Object};

use std::path::PathBuf;
use std::sync::Arc;

use super::Results;
use super::args::{Kind as Param, Spec, check_arguments, column, encryption_name};
use crate::ace_error;
use crate::dictionary_write::rights::{ALTER, DROP};
use crate::dictionary_write::*;
use crate::dictionary_write::{
    add_refintegrity, comment_value, decode_membership, flag_on, login_salts, nul_bytes,
    open_link_target, rebuild_dictionary, set_object_info1, set_object_info2, set_object_presence,
    strip_relative, under_folder, write_object_name, write_object_slots, write_user_membership,
};
use crate::handle::Handle;
use crate::table;
use crate::{Error, Session, connection, lock};

const ADMINISTRATOR: &str = "ADSSYS";

const SPECS: &[Spec] = &[
    Spec {
        name: "SP_CREATEGROUP",
        params: &[("GroupName", Param::Char(100)), ("Comment", Param::Memo)],
        required: 2,
        results: false,
    },
    Spec {
        name: "SP_CREATEUSER",
        params: &[
            ("UserName", Param::Char(50)),
            ("Password", Param::Char(20)),
            ("Comment", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_DROPUSER",
        params: &[("UserName", Param::Char(50))],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_DROPGROUP",
        params: &[("GroupName", Param::Char(100))],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_ADDUSERTOGROUP",
        params: &[
            ("UserName", Param::Char(50)),
            ("GroupName", Param::Char(100)),
        ],
        required: 2,
        results: false,
    },
    Spec {
        name: "SP_REMOVEUSERFROMGROUP",
        params: &[
            ("UserName", Param::Char(50)),
            ("GroupName", Param::Char(100)),
        ],
        required: 2,
        results: false,
    },
    Spec {
        name: "SP_CREATEREFERENTIALINTEGRITY",
        params: &[
            ("Name", Param::Char(200)),
            ("PrimaryTable", Param::Char(200)),
            ("ForeignTable", Param::Char(200)),
            ("ForeignKey", Param::Char(200)),
            ("UpdateRule", Param::Integer),
            ("DeleteRule", Param::Integer),
            ("FailTable", Param::Char(200)),
            ("PrimaryKeyError", Param::Memo),
            ("CascadeError", Param::Memo),
        ],
        required: 9,
        results: false,
    },
    Spec {
        name: "SP_DROPREFERENTIALINTEGRITY",
        params: &[("Name", Param::Char(200))],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_CREATELINK",
        params: &[
            ("Name", Param::Char(200)),
            ("Dictionary", Param::Char(512)),
            ("Global", Param::Bool),
            ("StaticPath", Param::Bool),
            ("AuthenticateActiveUser", Param::Bool),
            ("UserName", Param::Char(200)),
            ("Password", Param::Char(20)),
        ],
        required: 7,
        results: false,
    },
    Spec {
        name: "SP_MODIFYLINK",
        params: &[
            ("Name", Param::Char(200)),
            ("Dictionary", Param::Char(512)),
            ("StaticPath", Param::Bool),
            ("AuthenticateActiveUser", Param::Bool),
            ("UserName", Param::Char(200)),
            ("Password", Param::Char(20)),
        ],
        required: 6,
        results: false,
    },
    Spec {
        name: "SP_DROPLINK",
        params: &[("Name", Param::Char(200)), ("Global", Param::Bool)],
        required: 2,
        results: false,
    },
    Spec {
        name: "SP_GETLINKS",
        params: &[],
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_GETSECURITYINFO",
        params: &[],
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_DISABLETRIGGERS",
        params: &[
            ("ObjectName", Param::Char(200)),
            ("Parent", Param::Char(200)),
            ("AllUsers", Param::Bool),
            ("Options", Param::Integer),
        ],
        required: 4,
        results: false,
    },
    Spec {
        name: "SP_ENABLETRIGGERS",
        params: &[
            ("ObjectName", Param::Char(200)),
            ("Parent", Param::Char(200)),
            ("AllUsers", Param::Bool),
            ("Options", Param::Integer),
        ],
        required: 4,
        results: false,
    },
    Spec {
        name: "SP_MODIFYGROUPPROPERTY",
        params: &[
            ("GroupName", Param::Char(100)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_MODIFYUSERPROPERTY",
        params: &[
            ("UserName", Param::Char(50)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_MODIFYVIEWPROPERTY",
        params: &[
            ("ViewName", Param::Char(200)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_MODIFYPROCEDUREPROPERTY",
        params: &[
            ("ProcedureName", Param::Char(200)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_MODIFYINDEXPROPERTY",
        params: &[
            ("TableName", Param::Char(200)),
            ("IndexName", Param::Char(200)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
        ],
        required: 4,
        results: false,
    },
    Spec {
        name: "SP_MODIFYTRIGGERPROPERTY",
        params: &[
            ("TriggerName", Param::Char(200)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_MODIFYDATABASE",
        params: &[("Property", Param::Char(200)), ("Value", Param::Memo)],
        required: 2,
        results: false,
    },
    Spec {
        name: "SP_ADDTABLETODATABASE",
        params: &[
            ("TableName", Param::Char(200)),
            ("TablePath", Param::Char(260)),
            ("TableType", Param::Integer),
            ("CharType", Param::Integer),
            ("IndexFiles", Param::Memo),
            ("Comment", Param::Memo),
        ],
        required: 6,
        results: false,
    },
    Spec {
        name: "SP_ADDINDEXFILETODATABASE",
        params: &[
            ("TableName", Param::Char(200)),
            ("IndexFilePath", Param::Char(260)),
            ("Comment", Param::Memo),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_REMOVEINDEXFILE",
        params: &[
            ("TableName", Param::Char(200)),
            ("IndexName", Param::Char(200)),
            ("DeleteFile", Param::Bool),
        ],
        required: 3,
        results: false,
    },
    Spec {
        name: "SP_RENAMEDDOBJECT",
        params: &[
            ("ObjectName", Param::Char(200)),
            ("NewObjectName", Param::Char(200)),
            ("ObjectType", Param::Integer),
            ("Options", Param::Integer),
        ],
        required: 4,
        results: false,
    },
    Spec {
        name: "SP_MOVEDDOBJECTFILE",
        params: &[
            ("ObjectType", Param::Integer),
            ("ObjectName", Param::Char(200)),
            ("NewFilePath", Param::Char(260)),
            ("IndexFiles", Param::Memo),
            ("Parent", Param::Char(200)),
            ("Options", Param::Integer),
        ],
        required: 6,
        results: false,
    },
    Spec {
        name: "SP_MODIFYTABLEPROPERTY",
        params: &[
            ("TableName", Param::Char(200)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
            ("ValidationOption", Param::Char(200)),
            ("FailTable", Param::Char(200)),
        ],
        required: 5,
        results: false,
    },
    Spec {
        name: "SP_MODIFYFIELDPROPERTY",
        params: &[
            ("TableName", Param::Char(200)),
            ("FieldName", Param::Char(200)),
            ("Property", Param::Char(200)),
            ("Value", Param::Memo),
            ("ValidationOption", Param::Char(200)),
            ("FailTable", Param::Char(200)),
        ],
        required: 6,
        results: false,
    },
    Spec {
        name: "SP_SETDDENCRYPTIONTYPE",
        params: &[
            ("DictionaryName", Param::Char(260)),
            ("AdsSysPW", Param::Char(20)),
            ("DDPassword", Param::Char(20)),
            ("EncryptionType", Param::Char(20)),
            ("Encrypt", Param::Bool),
        ],
        required: 5,
        results: false,
    },
    Spec {
        name: "SP_CHANGECURRENTUSERPASSWORD",
        params: &[
            ("OldPassword", Param::Char(20)),
            ("NewPassword", Param::Char(20)),
        ],
        required: 2,
        results: false,
    },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SPECS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(name))
}

/// Whether `name` is one of these procedures.
pub(crate) fn handles(name: &str) -> bool {
    spec(name).is_some()
}

/// Whether the procedure `name` has a result set to select from.
pub(crate) fn has_results(name: &str) -> bool {
    spec(name).is_some_and(|spec| spec.results)
}

/// The parameters of the procedure `name`, as fields: what types a call's
/// parameters (`?`) get.
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    spec(name).map_or_else(Vec::new, super::args::inputs)
}

/// The `ADS_DD_*_OBJECT` types of what a procedure's user needs rights on.
const TABLE: u16 = 1;
const USER: u16 = 8;
const GROUP: u16 = 9;
const LINK: u16 = 12;

/// 5054 with what the procedure refuses.
fn not_permitted(detail: &str) -> Error {
    ace_error::sql(5054, detail)
}

/// What a procedure runs on: the connection's dictionary and its arguments.
struct Call<'a> {
    session: &'a Session,
    connection: Handle,
    conn: Arc<connection::Connection>,
    add_path: PathBuf,
    args: Vec<V>,
}

impl Call<'_> {
    /// Argument `i` as text, NULL for none.
    fn text(&self, i: usize) -> Option<&str> {
        match self.args.get(i) {
            Some(V::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// Argument `i` as an Integer parameter reads it: a number is cut to a
    /// whole one (`fixtures/sql/spddnum.json`).
    fn integer(&self, i: usize) -> Option<i64> {
        super::args::whole(self.args.get(i))
    }

    /// Whether the connection's user may change the dictionary's users and
    /// groups: the administrator and the members of `DB:Admin`.
    fn administrator(&self) -> bool {
        self.conn
            .with_database_opt(crate::link::may_create)
            .unwrap_or(true)
    }

    /// Whether the connection's user may create objects of the type
    /// `object_type`: the administrator, a member of `DB:Admin` and a user
    /// the right was granted to.
    fn may_create(&self, object_type: u16) -> bool {
        self.administrator()
            || self
                .conn
                .with_database_opt(|db| db.may_create_type(object_type))
                .unwrap_or(true)
    }

    /// Whether the connection's user holds the right `bit` on the object of
    /// `object_type` named `name`, or is one of those who hold all.
    fn may(&self, object_type: u16, name: &str, bit: u32) -> bool {
        self.administrator()
            || self
                .conn
                .with_database_opt(|db| db.may(object_type, name, bit))
                .unwrap_or(true)
    }

    fn dictionary(&self) -> Result<Dictionary, Error> {
        open_dict(&self.add_path)
    }

    /// What the original has done by the time it refuses a name that is taken:
    /// it has taken the next object id, and stored the object's description,
    /// which it leaves.
    fn burn(&self, description: &str) -> Result<(), Error> {
        let memo = dd::memo_path(&self.add_path);
        let mut record = vec![0u8; dd::create::RECORD_LENGTH];
        dd::create::set_description(&mut record, &memo, description.as_bytes())
            .map_err(|_| Error::CorruptTable)?;
        bump_next_id(&self.add_path, false)
    }
}

/// Runs the procedure `execute` calls with `args`, on `connection`.
pub(crate) fn run(
    session: &Session,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    let spec = spec(&execute.name).ok_or(Error::NotImplemented)?;
    if spec.name == "SP_GETLINKS" {
        return get_links(session, connection);
    }
    let arguments = check_arguments(spec, execute, args)?;
    if spec.name == "SP_GETSECURITYINFO" {
        return get_security_info(session, connection).map(Some);
    }
    // The encryption of a dictionary is asked of a connection to a folder as
    // well, whose path it looks under.
    let (conn, add_path) = match spec.name {
        "SP_SETDDENCRYPTIONTYPE" => {
            let conn = connection::get(session, connection)?;
            let path = conn.path.clone();
            (conn, path)
        }
        _ => dictionary_connection(session, connection)
            .map_err(|_| Error::Sql(SqlError::no_dictionary()))?,
    };
    let call = Call {
        session,
        connection,
        conn,
        add_path,
        args: arguments.values.clone(),
    };
    match spec.name {
        "SP_CREATEGROUP" => create_group(&call),
        "SP_CREATEUSER" => create_user(&call),
        "SP_DROPGROUP" => drop_group(&call),
        "SP_DROPUSER" => drop_user(&call),
        "SP_ADDUSERTOGROUP" => membership(&call, true),
        "SP_REMOVEUSERFROMGROUP" => membership(&call, false),
        "SP_CHANGECURRENTUSERPASSWORD" => change_password(&call),
        "SP_MODIFYGROUPPROPERTY" => modify_group_property(&call),
        "SP_MODIFYUSERPROPERTY" => modify_user_property(&call),
        "SP_MODIFYVIEWPROPERTY" => modify_view_property(&call),
        "SP_MODIFYPROCEDUREPROPERTY" => modify_procedure_property(&call),
        "SP_MODIFYINDEXPROPERTY" => modify_index_property(&call),
        "SP_MODIFYTRIGGERPROPERTY" => modify_trigger_property(&call),
        "SP_MODIFYDATABASE" => modify_database(&call),
        "SP_SETDDENCRYPTIONTYPE" => set_encryption_type(&call),
        "SP_MODIFYTABLEPROPERTY" => modify_table_property(&call),
        "SP_MODIFYFIELDPROPERTY" => modify_field_property(&call),
        "SP_MOVEDDOBJECTFILE" => move_object_file(&call),
        "SP_RENAMEDDOBJECT" => rename_object(&call),
        "SP_ADDTABLETODATABASE" => add_table(&call),
        "SP_ADDINDEXFILETODATABASE" => add_index_file(&call),
        "SP_REMOVEINDEXFILE" => remove_index_file(&call),
        "SP_DISABLETRIGGERS" => triggers(&call, true),
        "SP_ENABLETRIGGERS" => triggers(&call, false),
        "SP_CREATELINK" => create_link(&call),
        "SP_MODIFYLINK" => modify_link(&call),
        "SP_DROPLINK" => drop_link(&call),
        "SP_CREATEREFERENTIALINTEGRITY" => create_ri(&call),
        "SP_DROPREFERENTIALINTEGRITY" => drop_ri(&call),
        _ => Err(Error::NotImplemented),
    }?;
    match arguments.cut {
        true => Err(Error::Sql(SqlError::input_too_large())),
        false => Ok(None),
    }
}

/// What a name of an object may not be, as the procedures refuse it, in this
/// order: empty (`empty` says it as the procedure does), with a `;` or `:`,
/// and for a user the administrator's.
fn check_name(name: &str, empty: &str, user: bool) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Err(not_valid(empty));
    }
    if name.contains([';', ':']) {
        return Err(not_valid(
            " ';' and ':' characters are not allowed in the object name.",
        ));
    }
    if user && name.trim().eq_ignore_ascii_case(ADMINISTRATOR) {
        return Err(not_valid(
            "The object name cannot be the same as the administrator name.",
        ));
    }
    Ok(())
}

/// The checks of the name of a new user or group.
fn check_new_name(call: &Call, name: &str, comment: &str, user: bool) -> Result<(), Error> {
    check_name(name, "The object name cannot be empty.", user)?;
    let dict = call.dictionary()?;
    let taken = [
        Kind::Table,
        Kind::View,
        Kind::Procedure,
        Kind::Group,
        Kind::User,
    ]
    .into_iter()
    .any(|kind| dict.named(kind, name.trim()).is_some());
    if taken {
        call.burn(comment)?;
        return Err(not_valid(
            "An object with the same name already exists in the data dictionary.",
        ));
    }
    Ok(())
}

fn create_group(call: &Call) -> Result<(), Error> {
    if !call.may_create(GROUP) {
        return Err(not_permitted(
            "Cannot create user group object in the data dictionary.",
        ));
    }
    let name = call.text(0).unwrap_or("");
    check_new_name(call, name, call.text(1).unwrap_or(""), false)?;
    create_group_api(
        call.session,
        call.connection,
        name.trim(),
        call.text(1).unwrap_or(""),
    )
    .map_err(ace_error::sql_error)
}

fn create_user(call: &Call) -> Result<(), Error> {
    if !call.may_create(USER) {
        return Err(not_permitted(
            "Cannot create user object in the data dictionary.",
        ));
    }
    let name = call.text(0).unwrap_or("");
    check_new_name(call, name, call.text(2).unwrap_or(""), true)?;
    create_user_api(
        call.session,
        call.connection,
        "",
        name.trim(),
        call.text(1).unwrap_or(""),
        call.text(2).unwrap_or(""),
    )
    .map_err(ace_error::sql_error)
}

fn drop_group(call: &Call) -> Result<(), Error> {
    let name = call.text(0).unwrap_or("").trim();
    if dd::built_in_group(name).is_some() {
        return Err(not_permitted(
            "Predefined database roles cannot be deleted.",
        ));
    }
    if call.dictionary()?.named(Kind::Group, name).is_none() {
        return Err(not_valid(
            "There is no such named user group in the data dictionary.",
        ));
    }
    if !call.may(GROUP, name, DROP) {
        return Err(not_permitted(""));
    }
    leave_group(call, name)?;
    delete_group_api(call.session, call.connection, name).map_err(ace_error::sql_error)
}

/// Takes the group `name` out of its members' memberships, as the original
/// does when it drops the group.
fn leave_group(call: &Call, name: &str) -> Result<(), Error> {
    let dict = call.dictionary()?;
    let Some(group) = dict.named(Kind::Group, name) else {
        return Ok(());
    };
    let Some(key) = call.conn.with_database_opt(|db| db.key()).flatten() else {
        return Ok(());
    };
    let id = group.id;
    let scheme = dict.scheme();
    let members: Vec<(u32, Vec<u32>)> = dict
        .all(Kind::User)
        .into_iter()
        .map(|user| (user.record(), decode_membership(scheme, user, &key)))
        .filter(|(_, ids)| ids.contains(&id))
        .collect();
    drop(dict);
    for (record, mut ids) in members {
        ids.retain(|&other| other != id);
        write_user_membership(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            &ids,
            scheme,
            &key,
        )?;
    }
    Ok(())
}

fn drop_user(call: &Call) -> Result<(), Error> {
    let name = call.text(0).unwrap_or("").trim();
    if call.dictionary()?.named(Kind::User, name).is_none() {
        return Err(not_valid("The specified user does not exist."));
    }
    if !call.may(USER, name, DROP) {
        return Err(not_permitted("Permission to drop the user is not granted."));
    }
    delete_user_api(call.session, call.connection, name).map_err(ace_error::sql_error)
}

/// `sp_AddUserToGroup` (`add`) and `sp_RemoveUserFromGroup`.
fn membership(call: &Call, add: bool) -> Result<(), Error> {
    let user = call.text(0).unwrap_or("").trim();
    let group = call.text(1).unwrap_or("").trim();
    if group.eq_ignore_ascii_case("DB:Public") {
        return Err(not_valid(match add {
            true => "The DB:Public role already includes all users.",
            false => "The DB:Public role always includes all users and it cannot be modified.",
        }));
    }
    let built_in = dd::built_in_group(group).is_some();
    let dict = call.dictionary()?;
    if !built_in && dict.named(Kind::Group, group).is_none() {
        return Err(not_valid(
            "There is no such named user group in the data dictionary.",
        ));
    }
    if !call.may(GROUP, group, ALTER) {
        return Err(not_permitted(""));
    }
    if dict.named(Kind::User, user).is_none() {
        return Err(not_valid(
            "There is no such named user in the data dictionary.",
        ));
    }
    drop(dict);
    let done = match add {
        true => add_user_to_group_api(call.session, call.connection, group, user),
        false => remove_user_from_group_api(call.session, call.connection, group, user),
    };
    // The calls name the user in these two; the procedures do not.
    done.map_err(|error| match error {
        Error::Sql(sql) if sql.native == 5157 => already_member(),
        Error::Sql(sql) if sql.native == 5156 => not_member(),
        other => ace_error::sql_error(other),
    })
}

fn already_member() -> Error {
    ace_error::sql(5157, "")
}

fn not_member() -> Error {
    ace_error::sql(5156, "")
}

/// `sp_ChangeCurrentUserPassword`: the connection's own user's.
fn change_password(call: &Call) -> Result<(), Error> {
    let bytes = |text: Option<&str>| text.unwrap_or("").as_bytes().to_vec();
    change_own_password(
        call.session,
        &call.conn,
        &call.add_path,
        &bytes(call.text(0)),
        &bytes(call.text(1)),
    )
    .map_err(ace_error::sql_error)
}

/// A rule of a referential integrity: 1 to 4 (cascade, restrict, set null,
/// set default).
fn rule(call: &Call, arg: usize) -> Result<u8, Error> {
    match call.integer(arg) {
        Some(n @ 1..=4) => Ok(n as u8),
        _ => Err(ace_error::sql(
            5127,
            match arg {
                4 => "Parameter 8 is not a valid update rule.",
                _ => "Parameter 9 is not a valid delete rule.",
            },
        )),
    }
}

/// `sp_CreateReferentialIntegrity`: what it checks, in this order, and the
/// refusals are the SQL engine's own (`fixtures/sql/spddri.json`).
fn create_ri(call: &Call) -> Result<(), Error> {
    let dict = call.dictionary()?;
    let parent = call.text(1).unwrap_or("").trim();
    let parent = dict
        .named(Kind::Table, parent)
        .ok_or_else(|| not_valid("There is no such named table in the data dictionary."))?;
    let primary = parent
        .info1
        .filter(|&id| id != 0)
        .and_then(|id| dict.object(id as u32))
        .ok_or_else(|| ace_error::sql(5138, ""))?;
    let (update, delete) = (rule(call, 4)?, rule(call, 5)?);
    // Refused before the foreign key table is looked for
    // (`fixtures/sql/spddri`, `ddrights`).
    let child = dict.named(Kind::Table, call.text(2).unwrap_or("").trim());
    if !(call.may(TABLE, &parent.name, ALTER)
        && child.is_none_or(|child| call.may(TABLE, &child.name, ALTER)))
    {
        return Err(not_permitted(""));
    }
    let child = child
        .ok_or_else(|| not_valid("The foreign key table name is not in the data dictionary."))?;
    let tag = dict
        .child(child.id, Kind::Index, call.text(3).unwrap_or("").trim())
        .ok_or_else(|| not_valid("The foreign key index is not in the data dictionary."))?;
    if primary.u16(dd::index::KEY_LENGTH) != tag.u16(dd::index::KEY_LENGTH) {
        return Err(ace_error::sql(
            5143,
            "The primary key and foreign key are not the same length.",
        ));
    }
    // The rows there already must keep to the rule, the first that does not
    // named (`fixtures/sql/spddri2.json`).
    let expression = tag.text(dd::index::EXPRESSION).unwrap_or_default();
    if !expression.contains(';') && !expression.contains('+') {
        let first = crate::ri::first_orphan(
            call.session,
            call.connection,
            &parent.name,
            &primary.name,
            &child.name,
            expression.trim(),
        )?;
        if let Some(record) = first {
            return Err(ace_error::sql(
                5139,
                &format!(
                    "The offending foreign key value found in row {record} of table \"{}\".",
                    child.name
                ),
            ));
        }
    }
    let name = call.text(0).unwrap_or("").trim();
    if name.is_empty() {
        return Err(not_valid("The object name cannot be empty."));
    }
    if dict.named(Kind::Relation, name).is_some() {
        call.burn("")?;
        return Err(not_valid(
            "An object with the same name already exists in the data dictionary.",
        ));
    }
    let (parent_name, primary_name) = (parent.name.clone(), primary.name.clone());
    let (child_name, tag_name) = (child.name.clone(), tag.name.clone());
    drop(dict);
    add_refintegrity(
        call.session,
        call.connection,
        [name, &parent_name, &primary_name, &child_name, &tag_name],
        (update, delete),
        (call.text(7).unwrap_or(""), call.text(8).unwrap_or("")),
        false,
    )
    .map_err(ace_error::sql_error)
}

fn drop_ri(call: &Call) -> Result<(), Error> {
    let name = call.text(0).unwrap_or("").trim();
    let dict = call.dictionary()?;
    let Some(relation) = dict.named(Kind::Relation, name) else {
        return Err(not_valid("The referential integrity could not be found."));
    };
    let tables = [dd::relation::PRIMARY_TABLE, dd::relation::FOREIGN_TABLE]
        .map(|slot| relation.u32(slot).and_then(|id| dict.object(id)));
    let changes = tables
        .iter()
        .flatten()
        .all(|table| call.may(TABLE, &table.name, ALTER));
    drop(dict);
    if !changes {
        return Err(not_permitted(""));
    }
    remove_refintegrity_api(call.session, call.connection, name).map_err(ace_error::sql_error)
}

/// A flag argument: 0 and NULL are off, 1 on, and so is any other number a
/// bound parameter gives.
fn flag(call: &Call, arg: usize) -> bool {
    call.integer(arg).is_some_and(|n| n != 0)
}

/// A link's name, refused as a name is.
fn check_link_name(call: &Call) -> Result<String, Error> {
    let name = call.text(0).unwrap_or("").trim().to_owned();
    check_name(&name, "The link name must be specified.", false)?;
    Ok(name)
}

/// 5020 for a dictionary's path longer than a path can be.
fn check_link_path(call: &Call, arg: usize) -> Result<(), Error> {
    match call
        .text(arg)
        .is_some_and(|path| path.chars().count() > 260)
    {
        true => Err(ace_error::sql(5020, "Path too long.")),
        false => Ok(()),
    }
}

fn create_link(call: &Call) -> Result<(), Error> {
    let name = check_link_name(call)?;
    if call.dictionary()?.named(Kind::Table, &name).is_some() {
        call.burn("")?;
        return Err(not_valid(&name));
    }
    check_link_path(call, 1)?;
    let options =
        u32::from(flag(call, 2)) | u32::from(flag(call, 4)) << 1 | u32::from(flag(call, 3)) << 2;
    let (path, user, password) = (
        call.text(1).unwrap_or(""),
        call.text(5).unwrap_or(""),
        call.text(6).unwrap_or(""),
    );
    // A link that logs in as the connection's user needs the dictionary to
    // know it.
    if options & crate::link::AUTH_ACTIVE_USER != 0 {
        let active = call
            .conn
            .with_database_opt(|db| db.user().map(str::to_owned))
            .flatten()
            .unwrap_or_default();
        open_link_target(&call.add_path, path, &active, 0)?;
    }
    let made = add_link_api(
        call.session,
        call.connection,
        &name,
        path,
        user,
        password,
        options,
    );
    // A link of the connection's own that does not log in is known to it all
    // the same, if not active.
    if matches!(&made, Err(Error::Sql(sql)) if sql.native == 5097) {
        lock(&call.conn.links).push(crate::link::Local {
            name,
            path: path.to_owned(),
            user: user.to_owned(),
            password: password.to_owned(),
            options,
        });
    }
    made.map_err(ace_error::sql_error)
}

/// A link of the dictionary, by its name.
fn find_link(call: &Call, name: &str) -> Result<Option<String>, Error> {
    Ok(call
        .dictionary()?
        .named(Kind::Link, name)
        .map(|link| link.name.clone()))
}

fn modify_link(call: &Call) -> Result<(), Error> {
    let name = check_link_name(call)?;
    let Some(name) = find_link(call, &name)? else {
        return Err(not_valid(
            "There is no such named link in the data dictionary.",
        ));
    };
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    check_link_path(call, 1)?;
    let options = u32::from(flag(call, 3)) << 1 | u32::from(flag(call, 2)) << 2;
    let (path, user) = (call.text(1).unwrap_or(""), call.text(4).unwrap_or(""));
    open_link_target(&call.add_path, path, user, options | crate::link::GLOBAL)?;
    modify_link_api(
        call.session,
        call.connection,
        &name,
        path,
        user,
        call.text(5).unwrap_or(""),
        options | crate::link::GLOBAL,
    )
    .map_err(ace_error::sql_error)
}

/// `sp_DropLink`: a global link leaves the dictionary; for any other the
/// connection stops using it, which fails once it has.
fn drop_link(call: &Call) -> Result<(), Error> {
    let name = call.text(0).unwrap_or("").trim();
    if flag(call, 1) {
        let Some(name) = find_link(call, name)? else {
            return Err(not_valid(
                "There is no such named link object in the data dictionary.",
            ));
        };
        if !call.may(LINK, &name, DROP) {
            return Err(not_permitted(""));
        }
        return drop_link_api(call.session, call.connection, &name).map_err(ace_error::sql_error);
    }
    let active = lock(&call.conn.active_links)
        .iter()
        .any(|link| link.name.eq_ignore_ascii_case(name));
    if active {
        crate::link::deactivate(&call.conn, name);
        return Ok(());
    }
    let known = lock(&call.conn.links)
        .iter()
        .any(|link| link.name.eq_ignore_ascii_case(name))
        || find_link(call, name)?.is_some();
    match known {
        true => Err(not_valid("The specified link is not active.")),
        false => Ok(()),
    }
}

/// `sp_GetLinks`: refused to every user, the administrator too.
fn get_links(session: &Session, connection: Handle) -> Result<Option<Results>, Error> {
    Err(match dictionary_connection(session, connection) {
        Ok(_) => ace_error::sql_bare(5054),
        Err(_) => ace_error::sql(5054, "sp_GetLinks"),
    })
}

/// `sp_GetSecurityInfo`: one row on how the connection is secured. Whether the
/// dictionary is encrypted is known to its administrator alone.
fn get_security_info(session: &Session, connection: Handle) -> Result<Results, Error> {
    use tflip_storage::field::FieldType;
    use tflip_storage::value::Value;
    let conn = connection::get(session, connection)?;
    let (encryption, administrator, encrypted) = conn
        .with_database_opt(|db| {
            let dictionary = db.dictionary();
            (db.opened_type(), db.is_admin(), dictionary.encrypted())
        })
        .unwrap_or((None, false, false));
    let encryption = encryption_name(encryption.unwrap_or(0));
    let padded = |text: &str, length: usize| Value::Text(format!("{text:<length$}"));
    Ok(Results::new(
        vec![
            column("FIPSMode", FieldType::Logical, 1),
            column("EncryptionType", FieldType::Character, 10),
            column("DictionaryEncrypted", FieldType::Logical, 1),
            column("CommType", FieldType::Character, 10),
            column("TLSCipher", FieldType::Character, 20),
            column("TLSVersion", FieldType::Character, 20),
        ],
        vec![vec![
            Value::Logical(false),
            padded(encryption, 10),
            match administrator {
                true => Value::Logical(encrypted),
                false => Value::Null,
            },
            padded(
                match crate::lock(&session.client_address).is_some() {
                    true => "TCP_IP",
                    false => "LOCAL",
                },
                10,
            ),
            Value::Null,
            Value::Null,
        ]],
    ))
}

/// `sp_DisableTriggers` and `sp_EnableTriggers`: a table's, a view's or a
/// trigger's own flag, or the dictionary's, which for the connection alone is
/// the connection's.
fn triggers(call: &Call, disable: bool) -> Result<(), Error> {
    if !matches!(call.integer(3), Some(0) | None) {
        return Err(ace_error::sql(5019, "Parameter 4"));
    }
    let name = call.text(0).unwrap_or("").trim();
    let parent = call.text(1).unwrap_or("").trim();
    let all_users = flag(call, 2);
    let dict = call.dictionary()?;
    let on = |kind: Kind, name: &str| {
        dict.named(kind, name)
            .map(|object| (object.record(), object.id, object.has(slot_of(kind))))
    };
    let database_off = dict
        .database()
        .is_some_and(|d| d.has(dd::database::TRIGGERS_DISABLED));
    // Nothing is changed inside what is disabled.
    let refuse = |detail: &str| ace_error::sql(if disable { 5196 } else { 5197 }, detail);
    // The table or view whose `ALTER` right a user who is not the
    // administrator needs, as `Database::may` takes it.
    let mut guarded: Option<(u16, String)> = None;
    // What the call is on: the dictionary, a table or view, or a trigger of
    // one, with the record to write and whether it is disabled.
    let target = match (name.is_empty(), parent.is_empty()) {
        (true, true) => None,
        (false, true) => {
            let found = on(Kind::Table, name)
                .map(|found| (Kind::Table, found))
                .or_else(|| on(Kind::View, name).map(|found| (Kind::View, found)));
            let Some((kind, found)) = found else {
                return Err(not_valid("No such table or view."));
            };
            guarded = Some((if kind == Kind::Table { 1 } else { 6 }, name.to_owned()));
            Some((kind, found))
        }
        (_, false) => {
            let owner = on(Kind::Table, parent)
                .map(|found| (Kind::Table, found))
                .or_else(|| on(Kind::View, parent).map(|found| (Kind::View, found)));
            let Some((owner_kind, owner)) = owner else {
                return Err(not_valid(
                    "There is no such named table in the data dictionary.",
                ));
            };
            guarded = Some((
                if owner_kind == Kind::Table { 1 } else { 6 },
                parent.to_owned(),
            ));
            if database_off {
                return Err(refuse("Entire dictionary has triggers disabled."));
            }
            if owner.2 {
                return Err(refuse("Entire table has triggers disabled."));
            }
            let trigger = dict
                .child(owner.1, Kind::Trigger, name)
                .filter(|_| !name.is_empty())
                .map(|t| (t.record(), t.id, t.has(slot_of(Kind::Trigger))));
            let Some(trigger) = trigger else {
                return Err(not_valid(
                    "There is no such named trigger in the data dictionary.",
                ));
            };
            Some((Kind::Trigger, trigger))
        }
    };
    // A user who is not the administrator may alter the triggers of a table
    // it may alter, for every user or for its connection, and enable its
    // connection's, but not disable them (`fixtures/sql/ddtrigrights`).
    if !call.administrator() {
        let permitted = match &guarded {
            Some((object_type, name)) => call
                .conn
                .with_database_opt(|db| db.may(*object_type, name, crate::system::right::ALTER))
                .unwrap_or(false),
            None => !all_users && !disable,
        };
        if !permitted {
            return Err(match target {
                None if !all_users => ace_error::sql(5161, ""),
                _ => not_permitted(""),
            });
        }
    }
    if target.is_some() && database_off {
        return Err(refuse("Entire dictionary has triggers disabled."));
    }
    let (kind, record, was) = match target {
        Some((kind, (record, _, was))) => (kind, record, was),
        None if all_users => {
            let database = dict.database().ok_or(Error::CorruptTable)?;
            (
                Kind::Database,
                database.record(),
                database.has(slot_of(Kind::Database)),
            )
        }
        None => {
            call.conn
                .triggers_disabled
                .store(disable, std::sync::atomic::Ordering::Relaxed);
            return Ok(());
        }
    };
    drop(dict);
    if was == disable {
        return Ok(());
    }
    write_object_slots(
        call.session,
        &call.conn,
        &call.add_path,
        record,
        &[(slot_of(kind), disable.then_some(&[][..]))],
    )
}

/// The property slot that holds an object's triggers-disabled flag.
fn slot_of(kind: Kind) -> usize {
    match kind {
        Kind::Table => dd::table::TRIGGERS_DISABLED,
        Kind::View => dd::view::TRIGGERS_DISABLED,
        Kind::Trigger => dd::trigger::TRIGGERS_DISABLED,
        _ => dd::database::TRIGGERS_DISABLED,
    }
}

/// 5126 for a property the object does not take, as the write underneath says it.
fn cannot_be_set(noun: &str) -> Error {
    ace_error::sql(
        5126,
        &format!("The specified property cannot be set for a {noun} object."),
    )
}

/// What a `sp_Modify…Property` procedure runs: the property's number and the
/// value as its type takes it.
struct Change {
    id: u32,
    value: Vec<u8>,
}

/// `Property` and `Value` (arguments `at` and `at + 1`) of a call that knows
/// the properties `names`.
fn change(call: &Call, at: usize, names: &[(&str, u32)]) -> Result<Change, Error> {
    let id = props::id_of(names, call.text(at).unwrap_or(""));
    let (_, value) = props::convert(id, call.text(at + 1))?;
    Ok(Change { id, value })
}

/// The properties `sp_ModifyGroupProperty` knows by name.
const GROUP_NAMES: &[(&str, u32)] = &[("COMMENT", 1), ("USER_DEFINED_PROP", 3)];

fn modify_group_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 1, GROUP_NAMES)?;
    let name = call.text(0).unwrap_or("").trim();
    let built_in = dd::built_in_group(name).is_some();
    if built_in && !name.eq_ignore_ascii_case("DB:Public") {
        return Err(not_permitted("Predefined database roles are read only."));
    }
    if !built_in && call.dictionary()?.named(Kind::Group, name).is_none() {
        return Err(not_valid(
            "There is no such named user group in the data dictionary.",
        ));
    }
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    if built_in {
        return Ok(());
    }
    match change.id {
        1 => {
            let record = call
                .dictionary()?
                .named(Kind::Group, name)
                .map(Object::record);
            set_comment_of(call, record.ok_or(Error::CorruptTable)?, &change.value)
        }
        3 => set_user_group_property_api(
            call.session,
            call.connection,
            name,
            change.id as u16,
            &change.value,
        )
        .map_err(ace_error::sql_error),
        _ => Err(cannot_be_set("user group")),
    }
}

/// Sets the comment of the object at `record`: the old one is freed before the
/// new one is stored, as the procedures do (the calls store it first,
/// `fixtures/sql/spddgprops`).
fn set_comment_of(call: &Call, record: u32, value: &[u8]) -> Result<(), Error> {
    let memo = dd::memo_path(&call.add_path);
    {
        let mut file = open_dictionary_file(&call.add_path)?;
        let (block, length) =
            dd::create::comment_ref(&call.add_path, record).map_err(|_| Error::CorruptTable)?;
        // A comment of as many blocks is written over the old one.
        let at = dd::create::COMMENT;
        if let Some(stored) =
            dd::create::rewrite_comment(&memo, block, length, comment_value(value))
                .map_err(|_| Error::CorruptTable)?
        {
            file.write_part(record, at + 4, &stored.to_le_bytes())
                .map_err(|_| Error::CorruptTable)?;
            drop(file);
            return rebuild_dictionary(call.session, &call.conn, &call.add_path);
        }
        // A comment that is cleared is not freed.
        if !comment_value(value).is_empty() {
            dd::create::free_comment(&memo, block, length).map_err(|_| Error::CorruptTable)?;
        }
        // A scratch record carries the new reference.
        let mut field = vec![0u8; dd::create::RECORD_LENGTH];
        dd::create::set_comment(&mut field, &memo, comment_value(value))
            .map_err(|_| Error::CorruptTable)?;
        let at = dd::create::COMMENT;
        file.write_part(record, at, &field[at..at + 8])
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(call.session, &call.conn, &call.add_path)
}

/// The properties `sp_ModifyUserProperty` knows by name.
const USER_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("USER_DEFINED_PROP", 3),
    ("USER_PASSWORD", 1101),
    ("ENABLE_INTERNET", 107),
    ("LOGINS_DISABLED", 113),
    ("REQUIRE_OLD_PASSWORD", 1105),
];

fn modify_user_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 1, USER_NAMES)?;
    let name = call.text(0).unwrap_or("").trim();
    // Who may set a password, and whether it asks for the old one, is the
    // password's own rule (`fixtures/sql/spddpwchange`).
    if change.id == 1101 {
        let password = call.text(2).unwrap_or("").as_bytes();
        return set_user_password(call.session, &call.conn, &call.add_path, name, password)
            .map_err(ace_error::sql_error);
    }
    if name.eq_ignore_ascii_case(ADMINISTRATOR) {
        return Err(ace_error::sql(
            5126,
            "The property cannot be set for the administrator.",
        ));
    }
    let record = {
        let dict = call.dictionary()?;
        let Some(user) = dict.named(Kind::User, name) else {
            return Err(not_valid(
                "There is no such named user in the data dictionary.",
            ));
        };
        user.record()
    };
    if !call.may(8, name, ALTER) {
        return Err(not_permitted(""));
    }
    match change.id {
        1 => set_comment_of(call, record, &change.value),
        3 => set_user_property_api(call.session, call.connection, name, 3, &change.value)
            .map_err(ace_error::sql_error),
        107 | 113 | 1105 if change.value.is_empty() => Err(buffer_size(
            "The property should be a 2 byte boolean value.",
        )),
        1105 => set_user_property_api(call.session, call.connection, name, 1105, &change.value)
            .map_err(ace_error::sql_error),
        107 | 113 => {
            let slot = match change.id {
                107 => dd::user::ENABLE_INTERNET,
                _ => dd::user::LOGINS_DISABLED,
            };
            set_object_slot(
                call.session,
                &call.conn,
                &call.add_path,
                record,
                slot,
                &change.value,
            )
            .map_err(ace_error::sql_error)
        }
        _ => Err(cannot_be_set("user")),
    }
}

/// The properties `sp_ModifyViewProperty` knows by name.
const VIEW_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("TRIGGERS_DISABLED", 5),
    ("VIEW_STMT", 700),
    ("VIEW_STMT_LEN", 701),
];

/// 5153 for a statement a view cannot have, with what is wrong.
fn invalid_statement(detail: &str) -> Error {
    ace_error::sql(5153, detail)
}

fn modify_view_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 1, VIEW_NAMES)?;
    let name = call.text(0).unwrap_or("").trim();
    let record = call
        .dictionary()?
        .named(Kind::View, name)
        .map(Object::record)
        .ok_or_else(|| not_valid("There is no such named view in the data dictionary."))?;
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    match change.id {
        1 => set_comment_of(call, record, &change.value),
        5 if change.value.is_empty() => Err(ace_error::sql(
            5133,
            "The property is a 16 bit Boolean value.",
        )),
        5 => set_object_presence(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            dd::view::TRIGGERS_DISABLED,
            flag_on(&change.value),
        )
        .map_err(ace_error::sql_error),
        700 => {
            use tflip_sql::ViewRefusal;
            let text = call.text(2).unwrap_or("");
            if text.is_empty() {
                return Err(invalid_statement(".\\cdiction.cpp, 12289"));
            }
            let view = tflip_sql::check_view_property(text).map_err(|refusal| match refusal {
                ViewRefusal::Sql(error) => Error::Sql(error.inner()),
                ViewRefusal::NotSelect => {
                    invalid_statement("The view definition must be a SELECT statement.")
                }
                ViewRefusal::Order => {
                    invalid_statement("The view definition cannot contain an ORDER BY clause.")
                }
            })?;
            crate::statement::check_select(
                call.session,
                call.connection,
                &view.select,
                view.params,
            )
            .map_err(|error| match ace_error::sql_error(error) {
                Error::Sql(sql) => Error::Sql(sql.inner()),
                other => other,
            })?;
            let statement = nul_bytes(&view.text);
            set_view_property_api(call.session, call.connection, name, 700, &statement)
                .map_err(ace_error::sql_error)
        }
        _ => Err(cannot_be_set("view")),
    }
}

/// The properties `sp_ModifyProcedureProperty` knows by name.
const PROCEDURE_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("PROC_INPUT", 800),
    ("PROC_OUTPUT", 801),
    ("PROC_DLL_NAME", 802),
    ("PROC_DLL_FUNCTION_NAME", 803),
    ("PROC_INVOKE_OPTION", 804),
];

/// 5012 for a list of parameters with a definition the dictionary cannot use:
/// they are `name,type[,length[,decimals]]`, each ended by `;`, as a table's
/// fields are; the empty parts of a definition are left out.
fn check_parameters(list: &str) -> Result<(), Error> {
    let definitions: Vec<&str> = list.split(';').collect();
    let last = definitions.len() - 1;
    for (i, definition) in definitions.iter().enumerate() {
        if definition.trim().is_empty() {
            if i == last && i > 0 {
                break;
            }
            return Err(Error::InvalidFieldDefinition(
                "There may be an empty field definition.".into(),
            ));
        }
        let tokens: Vec<&str> = definition
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .collect();
        let cleaned = tokens.join(",");
        if tokens.len() < 2 {
            let comma = if definition.trim_end().ends_with(',') {
                ","
            } else {
                ""
            };
            let end = if i < last { ";" } else { "" };
            return Err(Error::InvalidFieldDefinition(format!(
                "Need to specify field type.  The problem field definition is: \"{cleaned}{comma}{end}\"."
            )));
        }
        crate::ddl::field_specs(&cleaned)?;
    }
    Ok(())
}

fn modify_procedure_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 1, PROCEDURE_NAMES)?;
    let name = call.text(0).unwrap_or("").trim();
    let record = call
        .dictionary()?
        .named(Kind::Procedure, name)
        .map(Object::record)
        .ok_or_else(|| {
            not_valid("There is no such named stored procedure in the data dictionary.")
        })?;
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    let text = call.text(2).unwrap_or("");
    match change.id {
        1 => set_comment_of(call, record, &change.value),
        800 | 801 => {
            if !text.is_empty() {
                check_parameters(text).map_err(ace_error::sql_error)?;
            }
            set_procedure_slot(call, record, change.id, &change.value, text.is_empty())
        }
        802 if text.chars().count() > 260 => Err(ace_error::sql(5005, "The path is too long.")),
        // An empty name is left as it was.
        802 if text.is_empty() => Ok(()),
        802 | 803 | 805 => {
            set_procedure_slot(call, record, change.id, &change.value, text.is_empty())
        }
        806 => Err(ace_error::sql(5133, ".\\cdiction.cpp, 12902")),
        _ => Err(cannot_be_set("procedure")),
    }
}

/// Sets a procedure's text property, which an empty text clears.
fn set_procedure_slot(
    call: &Call,
    record: u32,
    id: u32,
    value: &[u8],
    clear: bool,
) -> Result<(), Error> {
    let slot = value_slot(Kind::Procedure, id);
    write_object_slots(
        call.session,
        &call.conn,
        &call.add_path,
        record,
        &[(slot, (!clear).then_some(value))],
    )
}

/// The properties `sp_ModifyIndexProperty` knows by name, which are not
/// numbers of properties it reads a value for but the comment.
const INDEX_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("INDEX_FILE_NAME", 400),
    ("INDEX_EXPRESSION", 401),
    ("INDEX_CONDITION", 402),
    ("INDEX_OPTIONS", 403),
    ("INDEX_KEY_LENGTH", 404),
    ("INDEX_FTS_MIN_LENGTH", 406),
    ("INDEX_FTS_DELIMITERS", 407),
    ("INDEX_FTS_NOISE", 408),
    ("INDEX_FTS_DROP_CHARS", 409),
    ("INDEX_FTS_CONDITIONAL_CHARS", 410),
    ("INDEX_COLLATION", 411),
];

/// `sp_ModifyIndexProperty`: only the comment, and the property is looked at
/// before the index is.
fn modify_index_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 2, INDEX_NAMES)?;
    if change.id != 1 {
        return Err(ace_error::sql(5126, ""));
    }
    let record = {
        let dict = call.dictionary()?;
        let table = dict
            .named(Kind::Table, call.text(0).unwrap_or("").trim())
            .ok_or_else(|| not_valid("There is no such named table in the data dictionary."))?;
        let table_id = table.id;
        if !call.administrator() {
            return Err(not_permitted(""));
        }
        dict.child(table_id, Kind::Index, call.text(1).unwrap_or("").trim())
            .map(Object::record)
            .ok_or_else(|| {
                not_valid("There is no such named index order in the data dictionary.")
            })?
    };
    set_comment_of(call, record, &change.value)
}

/// The properties `sp_ModifyTriggerProperty` knows by name.
const TRIGGER_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("TRIGGERS_DISABLED", 5),
    ("TRIG_TABLEID", 1400),
    ("TRIG_EVENT_TYPE", 1401),
    ("TRIG_TRIGGER_TYPE", 1402),
    ("TRIG_CONTAINER_TYPE", 1403),
    ("TRIG_CONTAINER", 1404),
    ("TRIG_FUNCTION_NAME", 1405),
    ("TRIG_PRIORITY", 1406),
    ("TRIG_OPTIONS", 1407),
    ("TRIG_TABLENAME", 1408),
    ("TRIG_CONTAINER_W", 1409),
];

/// A trigger by its table's name and its own, `table::trigger`.
fn trigger_named<'a>(dict: &'a Dictionary, qualified: &str) -> Option<&'a Object> {
    let (table, trigger) = qualified.split_once("::")?;
    let table = dict.named(Kind::Table, table.trim())?;
    dict.child(table.id, Kind::Trigger, trigger.trim())
}

/// `sp_ModifyTriggerProperty`: a trigger by its table's name and its own,
/// `table::trigger`.
fn modify_trigger_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 1, TRIGGER_NAMES)?;
    // What no property of a trigger is, the write underneath refuses first.
    if matches!(change.id, 3 | 65535) {
        return Err(ace_error::sql(5131, ""));
    }
    let qualified = call.text(0).unwrap_or("");
    let record = {
        let dict = call.dictionary()?;
        trigger_named(&dict, qualified)
            .map(Object::record)
            .ok_or_else(|| not_valid("There is no such named trigger in the data dictionary."))?
    };
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    match change.id {
        5 if change.value.is_empty() => Err(ace_error::sql(
            5133,
            "The property is a 16 bit Boolean value.",
        )),
        5 => set_object_presence(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            dd::trigger::TRIGGERS_DISABLED,
            flag_on(&change.value),
        )
        .map_err(ace_error::sql_error),
        // An empty script is left as it was.
        1404 if call.text(2).unwrap_or("").is_empty() => Ok(()),
        // The wide form is stored as the original stores it, which tflip does not.
        1409 => Err(Error::NotImplemented),
        1404 => set_object_slot(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            dd::trigger::CONTAINER,
            &change.value,
        )
        .map_err(ace_error::sql_error),
        _ => Err(cannot_be_set("trigger")),
    }
}

/// The properties `sp_ModifyDatabase` knows by name.
const DATABASE_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("USER_DEFINED_PROP", 3),
    ("TRIGGERS_DISABLED", 5),
    ("DEFAULT_TABLE_PATH", 100),
    ("ADMIN_PASSWORD", 101),
    ("TEMP_TABLE_PATH", 102),
    ("LOG_IN_REQUIRED", 103),
    ("VERIFY_ACCESS_RIGHTS", 104),
    ("ENCRYPT_TABLE_PASSWORD", 105),
    ("ENCRYPT_NEW_TABLE", 106),
    ("ENABLE_INTERNET", 107),
    ("INTERNET_SECURITY_LEVEL", 108),
    ("MAX_FAILED_ATTEMPTS", 109),
    ("VERSION_MAJOR", 111),
    ("VERSION_MINOR", 112),
    ("LOGINS_DISABLED", 113),
    ("LOGINS_DISABLED_MSG", 114),
    ("FTS_DELIMITERS", 115),
    ("FTS_NOISE_WORDS", 116),
    ("FTS_DROP_CHARS", 117),
    ("FTS_CONDITIONAL_CHARS", 118),
    ("ENCRYPT_INDEXES", 120),
    ("QUERY_LOG_TABLE", 121),
    ("ENCRYPT_COMMUNICATION", 122),
    ("DISABLE_DLL_CACHING", 125),
    ("DATA_ENCRYPTION_TYPE", 126),
    ("QUERY_VIA_ROOT", 131),
    ("ENFORCE_MAX_FAILED_LOGINS", 132),
];

/// 5133 for a property given no bytes, and what size it is.
fn buffer_size(what: &str) -> Error {
    ace_error::sql(5133, what)
}

/// 5127 for a value a property does not take.
fn invalid_value(what: &str) -> Error {
    ace_error::sql(5127, what)
}

fn modify_database(call: &Call) -> Result<(), Error> {
    use dd::database as db;
    let change = change(call, 0, DATABASE_NAMES)?;
    let record = call
        .dictionary()?
        .database()
        .map(Object::record)
        .ok_or(Error::CorruptTable)?;
    // Whoever may not change it is told so, whatever else they may do.
    if change.id == 101 {
        return set_admin_password(call.session, &call.conn, &call.add_path, &change.value)
            .map_err(ace_error::sql_error);
    }
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    let id = change.id;
    let value = &change.value;
    let text = call.text(1).unwrap_or("");
    let two_byte = |what: &'static str| match value.len() {
        2 => Ok(()),
        _ => Err(buffer_size(what)),
    };
    let flag_bytes = |what: &'static str| two_byte(what);
    let slot_value = |slot: usize| -> Result<(), Error> {
        set_object_slot(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            slot,
            value,
        )
        .map_err(ace_error::sql_error)
    };
    let presence = |slot: usize, present: bool| -> Result<(), Error> {
        set_object_presence(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            slot,
            present,
        )
        .map_err(ace_error::sql_error)
    };
    let boolean = "The property should be a 2 byte boolean value.";
    let capital = "The property should be a 2 Byte boolean value.";
    let aes = call
        .conn
        .with_database_opt(|db| db.scheme() != tflip_storage::crypto::Scheme::Rc4)
        .unwrap_or(false);
    match id {
        1 => set_comment_of(call, record, value),
        3 => set_database_property_api(call.session, call.connection, 3, value)
            .map_err(ace_error::sql_error),
        5 => {
            flag_bytes(capital)?;
            presence(db::TRIGGERS_DISABLED, flag_on(value))
        }
        103 => {
            flag_bytes(boolean)?;
            match flag_on(value) {
                true => presence(db::LOGIN_NOT_REQUIRED, false),
                // The marker is 20 bytes the engine enciphers; only that it is
                // there matters.
                false => set_object_slot(
                    call.session,
                    &call.conn,
                    &call.add_path,
                    record,
                    db::LOGIN_NOT_REQUIRED,
                    &login_salts().slot4,
                )
                .map_err(ace_error::sql_error),
            }
        }
        104 => {
            flag_bytes(boolean)?;
            presence(db::RIGHTS_NOT_CHECKED, !flag_on(value))
        }
        // In a dictionary of AES tables the dictionary password is their key,
        // so new tables can be encrypted (`fixtures/aes/dictwrite`); one of
        // RC4 tables wants a table password (`fixtures/aes/rc4tp`).
        106 | 120 => {
            flag_bytes(boolean)?;
            let slot = if id == 106 {
                db::ENCRYPT_NEW_TABLE
            } else {
                db::ENCRYPT_INDEXES
            };
            crate::dictionary_write::rc4_tables::set_flag(
                call.session,
                &call.conn,
                &call.add_path,
                slot,
                flag_on(value),
            )
        }
        107 | 113 | 132 => {
            flag_bytes(match id {
                107 => capital,
                132 => "The property should be a 2 byte unsigned integer value.",
                _ => boolean,
            })?;
            slot_value(value_slot(Kind::Database, id))
        }
        122 | 125 => {
            flag_bytes(if id == 125 { capital } else { boolean })?;
            presence(value_slot(Kind::Database, id), flag_on(value))
        }
        108 => {
            two_byte("The property should be a 2 byte integer value.")?;
            match u16::from_le_bytes([value[0], value[1]]) {
                0..=2 => slot_value(value_slot(Kind::Database, id)),
                _ => Err(invalid_value("Invalid security level specified.")),
            }
        }
        109 => {
            two_byte("The property should be a 2 byte unsigned integer value.")?;
            slot_value(value_slot(Kind::Database, id))
        }
        111 | 112 => {
            two_byte("The property must be 16-bit value.")?;
            slot_value(value_slot(Kind::Database, id))
        }
        114 | 115 | 116 | 117 | 118 | 121 => {
            let slot = value_slot(Kind::Database, id);
            write_object_slots(
                call.session,
                &call.conn,
                &call.add_path,
                record,
                &[(slot, (!text.is_empty()).then_some(value.as_slice()))],
            )
            .map_err(ace_error::sql_error)
        }
        126 => {
            if value.len() != 4 {
                return Err(buffer_size(
                    "The property should be a 4 byte integer value.",
                ));
            }
            use crate::dictionary_write::convert::{Target, set_data_encryption_type};
            let target = match u32::from_le_bytes([value[0], value[1], value[2], value[3]]) {
                // RC4 is what a dictionary of RC4 tables has already.
                3 if !aes => return Ok(()),
                3 => Target::Rc4,
                5 => Target::Aes(tflip_storage::adt::ENCRYPTION_AES128),
                6 => Target::Aes(tflip_storage::adt::ENCRYPTION_AES256),
                _ => return Err(invalid_value("Invalid data encryption type specified.")),
            };
            set_data_encryption_type(call.session, &call.conn, &call.add_path, target)
        }
        131 => {
            if value.len() != 4 {
                return Err(buffer_size(
                    "The property should be a 4 byte integer value.",
                ));
            }
            match u32::from_le_bytes([value[0], value[1], value[2], value[3]]) {
                0..=3 => slot_value(value_slot(Kind::Database, id)),
                _ => Err(invalid_value("The value is out of range.")),
            }
        }
        105 => crate::dictionary_write::convert::set_table_password(
            call.session,
            &call.conn,
            &call.add_path,
            // The text with its NUL.
            value.strip_suffix(&[0]).unwrap_or(value),
        ),
        // The paths: not written.
        100 | 102 => Err(Error::NotImplemented),
        _ => Err(ace_error::sql(
            5126,
            "The specified property cannot be set for the database object.",
        )),
    }
}

/// The write's error as the procedures of tables word it: a missing file is
/// 7041, and an index file that does not fit its table has an identifier
/// the table does not know.
fn table_error(error: Error) -> Error {
    match error.plain() {
        Error::FileNotFound => {
            let detail = match &error {
                Error::Detailed(_, detail) => detail.as_str(),
                _ => "",
            };
            file_not_found(detail)
        }
        Error::InvalidFieldNumber => ace_error::sql(3011, ""),
        _ => ace_error::sql_error(error),
    }
}

/// 7041.
fn file_not_found(detail: &str) -> Error {
    ace_error::sql(7041, detail)
}

/// Whether the file at `path` is an ADT table.
fn is_adt_file(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok()
        && &head == b"Advantage Table\0"
}

/// `sp_AddTableToDatabase`: a free table (and the index files named) into the
/// dictionary.
fn add_table(call: &Call) -> Result<(), Error> {
    use crate::table::TableType;
    let table_type = match call.integer(2) {
        Some(1) => TableType::Ntx,
        Some(2) => TableType::Cdx,
        Some(3) => TableType::Adt,
        Some(4) => TableType::Vfp,
        _ => {
            // The position of the argument is told after the description's
            // first sentence, where the call underneath has the rest.
            return Err(Error::Sql(SqlError::ace(
                5025,
                "An invalid table type was specified.",
                "Valid choices are ADS_ADT, ADS_VFP, ADS_CDX, and ADS_NTX.  The table type \
                 must support the memo file type to be opened, if applicable.  Parameter 4",
            )));
        }
    };
    let char_type = match call.integer(3) {
        Some(n @ 1..=3) => n as u16,
        _ => {
            return Err(ace_error::sql(5027, "Parameter 5"));
        }
    };
    let mut path = call.text(1).unwrap_or("").trim().to_owned();
    // A path without a name is the type's default extension on no name.
    if path.is_empty() {
        path = match table_type {
            TableType::Adt => ".adt".to_owned(),
            _ => ".dbf".to_owned(),
        };
    }
    let mut name = call.text(0).unwrap_or("").trim().to_owned();
    if name.is_empty() {
        name = std::path::Path::new(&path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    let folder = dictionary_folder(&call.add_path).to_path_buf();
    let file = folder.join(&path);
    if !file.exists() {
        return Err(file_not_found(&file.display().to_string()));
    }
    // A table of the other format is no table of this one.
    if table_type != TableType::Adt && is_adt_file(&file) {
        return Err(ace_error::sql(7014, &file.display().to_string()));
    }
    if !call.administrator() {
        return Err(not_permitted(
            "Cannot create table object in the data dictionary.",
        ));
    }
    if table_type == TableType::Adt && dd::create::is_bound(&file).unwrap_or(false) {
        return Err(ace_error::sql(
            5130,
            "The table is not a free table.  It is already associated with a data dictionary.",
        ));
    }
    let dict = call.dictionary()?;
    let taken = [Kind::Table, Kind::View]
        .into_iter()
        .any(|kind| dict.named(kind, &name).is_some());
    drop(dict);
    if taken {
        call.burn(call.text(5).unwrap_or(""))?;
        return Err(not_valid(
            "An object with the same name already exists in the data dictionary.",
        ));
    }
    let mut indexes = call.text(4).unwrap_or("").to_owned();
    // An empty name in the list is a file with none.
    if !indexes.is_empty() && indexes.split(';').any(|name| name.trim().is_empty()) {
        return Err(file_not_found(""));
    }
    // An ADT table's own index file goes with it.
    if table_type == TableType::Adt {
        let own = std::path::Path::new(&path).with_extension("adi");
        let own = own.to_string_lossy().into_owned();
        let listed = indexes
            .split(';')
            .any(|name| name.trim().eq_ignore_ascii_case(&own));
        if folder.join(&own).exists() && !listed {
            indexes = match indexes.is_empty() {
                true => own,
                false => format!("{own};{indexes}"),
            };
        }
    }
    add_table_api(
        call.session,
        call.connection,
        &name,
        &path,
        table_type,
        char_type,
        &indexes,
        "",
    )
    .map_err(table_error)?;
    let comment = call.text(5).unwrap_or("");
    if comment.is_empty() {
        return Ok(());
    }
    let record = call
        .dictionary()?
        .named(Kind::Table, &name)
        .map(Object::record)
        .ok_or(Error::CorruptTable)?;
    set_description_of(call, record, comment)
}

/// Stores the comment `text` of the object at `record` as its description,
/// the bare text a call that makes the object stores: a comment set later
/// has its terminator.
fn set_description_of(call: &Call, record: u32, text: &str) -> Result<(), Error> {
    if text.is_empty() {
        return Ok(());
    }
    let mut field = vec![0u8; dd::create::RECORD_LENGTH];
    dd::create::set_description(&mut field, &dd::memo_path(&call.add_path), text.as_bytes())
        .map_err(|_| Error::CorruptTable)?;
    {
        let mut file = open_dictionary_file(&call.add_path)?;
        let at = dd::create::COMMENT;
        file.write_part(record, at, &field[at..at + 8])
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(call.session, &call.conn, &call.add_path)
}

/// `sp_AddIndexFileToDatabase`.
fn add_index_file(call: &Call) -> Result<(), Error> {
    let table = call.text(0).unwrap_or("").trim();
    if call.dictionary()?.named(Kind::Table, table).is_none() {
        return Err(not_valid(
            "There is no such named table in the data dictionary.",
        ));
    }
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    let path = call.text(1).unwrap_or("").trim();
    if path.is_empty() {
        return Err(file_not_found(""));
    }
    {
        let dict = call.dictionary()?;
        let parent = dict.named(Kind::Table, table).map(|t| t.id);
        let wanted = format!(".\\{path}");
        let open = parent.is_some_and(|parent| {
            dict.children(parent, Kind::IndexFile).iter().any(|index| {
                index
                    .text(dd::index_file::RELATIVE_PATH)
                    .is_some_and(|stored| {
                        stored.trim_end_matches('\0').eq_ignore_ascii_case(&wanted)
                    })
            })
        });
        if open {
            return Err(Error::Sql(SqlError::ace(
                5065,
                "The index is already open.",
                "It cannot be opened more than once.",
            )));
        }
    }
    add_index_file_api(
        call.session,
        call.connection,
        table,
        path,
        call.text(2).unwrap_or(""),
    )
    .map_err(table_error)?;
    let comment = call.text(2).unwrap_or("");
    if comment.is_empty() {
        return Ok(());
    }
    let record = {
        let dict = call.dictionary()?;
        let parent = dict.named(Kind::Table, table).map(|t| t.id);
        parent
            .and_then(|parent| dict.child(parent, Kind::IndexFile, path))
            .map(Object::record)
            .ok_or(Error::CorruptTable)?
    };
    set_description_of(call, record, comment)
}

/// `sp_RemoveIndexFile`.
fn remove_index_file(call: &Call) -> Result<(), Error> {
    let table = call.text(0).unwrap_or("").trim();
    let file = call.text(1).unwrap_or("").trim();
    let delete = flag(call, 2);
    {
        let dict = call.dictionary()?;
        let Some(parent) = dict.named(Kind::Table, table) else {
            return Err(not_valid(
                "There is no such named table in the data dictionary.",
            ));
        };
        if dict.child(parent.id, Kind::IndexFile, file).is_none() {
            return Err(not_valid("The index file is not part of the database."));
        }
    }
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    remove_index_file_api(call.session, call.connection, table, file, delete)
        .map_err(ace_error::sql_error)
}

/// `sp_RenameDDObject`: the kinds of object it renames, by the number of their
/// type, and the noun the error for one that is not there uses.
fn renamed_kind(object_type: i64) -> Option<(Kind, &'static str)> {
    Some(match object_type {
        1 => (Kind::Table, "table"),
        2 => (Kind::Relation, "referential integrity"),
        6 => (Kind::View, "view"),
        8 => (Kind::User, "user"),
        9 => (Kind::Group, "user group"),
        10 => (Kind::Procedure, "stored procedure"),
        12 => (Kind::Link, "link"),
        14 => (Kind::Trigger, "trigger"),
        15 => (Kind::Publication, "publication"),
        17 => (Kind::Subscription, "subscription"),
        18 => (Kind::Function, "function"),
        19 => (Kind::Package, "package"),
        _ => return None,
    })
}

fn invalid_object_type(detail: &str) -> Error {
    ace_error::sql(5136, detail)
}

fn rename_object(call: &Call) -> Result<(), Error> {
    let old = call.text(0).unwrap_or("").trim();
    rename_named(call, old)
}

/// `AdsDDRenameObject`: the same rename as the procedure, but the name of the
/// object is taken as it is given, its trailing blanks aside.
pub(crate) fn rename_dd_object(
    session: &Session,
    connection: Handle,
    old: &str,
    new: &str,
    object_type: u16,
    options: u32,
) -> Result<(), Error> {
    let (conn, add_path) =
        dictionary_connection(session, connection).map_err(|error| match error {
            Error::InvalidConnection => ace_error::plain(Error::Sql(SqlError::no_dictionary())),
            error => error,
        })?;
    let call = Call {
        session,
        connection,
        conn,
        add_path,
        args: vec![
            V::Str(old.to_owned()),
            V::Str(new.to_owned()),
            V::Int(i64::from(object_type)),
            V::Int(i64::from(options)),
        ],
    };
    rename_named(&call, old.trim_end()).map_err(ace_error::plain)
}

fn rename_named(call: &Call, old: &str) -> Result<(), Error> {
    let object_type = call.integer(2).unwrap_or(-1);
    if object_type == 11 {
        return Err(invalid_object_type("Invalid object type specified."));
    }
    let Some((kind, noun)) = renamed_kind(object_type) else {
        return Err(invalid_object_type("Parameter 4."));
    };
    // Only a table takes an option: to leave its files as they are.
    let options = call.integer(3).unwrap_or(0);
    if !(options == 0 || options == 1 && kind == Kind::Table) {
        return Err(ace_error::sql(5019, "Parameter 5."));
    }
    let given = call.text(1).unwrap_or("");
    // A trigger's new name may be qualified by a table's, which only names
    // the trigger: what follows the first `::` is its name.
    let qualified = kind == Kind::Trigger && given.contains("::");
    if !qualified {
        check_name(
            given,
            "The new name cannot be an empty string.",
            kind == Kind::User,
        )?;
    } else if given.trim().is_empty() {
        return Err(not_valid("The new name cannot be an empty string."));
    }
    let new = match qualified {
        true => given
            .split_once("::")
            .map_or(given, |(_, name)| name)
            .trim(),
        false => given.trim(),
    };
    if qualified {
        if new.is_empty() {
            return Err(ace_error::sql(
                5138,
                "The new trigger name cannot be empty string.",
            ));
        }
        check_name(new, "", false)?;
    }
    let dict = call.dictionary()?;
    let taken = match kind {
        // A trigger may be given the name it has, but not as a qualified one.
        Kind::Trigger if qualified => trigger_named(&dict, given.trim()).is_some(),
        Kind::Trigger => {
            let own = trigger_named(&dict, old);
            dict.named(kind, new)
                .is_some_and(|other| own.is_none_or(|own| own.record() != other.record()))
        }
        // Publications and subscriptions are named in the one space with the
        // tables and the rest, so those refuse their names; a relation's name
        // clashes with its own kind and with them alone.
        Kind::Relation => [Kind::Relation, Kind::Publication, Kind::Subscription]
            .into_iter()
            .any(|other| dict.named(other, new).is_some()),
        _ => {
            [
                Kind::Table,
                Kind::View,
                Kind::Procedure,
                Kind::Group,
                Kind::User,
                Kind::Link,
                Kind::Function,
                Kind::Package,
                Kind::Publication,
                Kind::Subscription,
            ]
            .into_iter()
            .any(|other| dict.named(other, new).is_some())
                || matches!(kind, Kind::Publication | Kind::Subscription)
                    && dict.named(Kind::Relation, new).is_some()
        }
    };
    if taken {
        return Err(not_valid(
            "An object with the same name already exists in the data dictionary.",
        ));
    }
    if kind == Kind::Group && dd::built_in_group(old).is_some() {
        return Err(not_permitted("Predefined database roles are read only."));
    }
    let missing = || match kind {
        Kind::Table => not_valid("The table could not be found in the data dictionary."),
        Kind::Function => not_valid("There is no such function in the database."),
        _ => not_valid(&format!(
            "There is no such named {noun} in the data dictionary."
        )),
    };
    // A trigger is named by its table's name and its own.
    let object = match kind {
        Kind::Trigger => trigger_named(&dict, old),
        _ => dict.named(kind, old),
    };
    // The administrator is in no record, but is no unknown user.
    if kind == Kind::User && old.eq_ignore_ascii_case(ADMINISTRATOR) {
        if !call.administrator() {
            return Err(not_permitted(""));
        }
        return Err(ace_error::sql(
            5126,
            "The property cannot be set for the administrator.",
        ));
    }
    let object = object.ok_or_else(missing)?;
    // The right to alter the object lets a user rename it, after the names
    // and the object were found (`fixtures/sql/ddfuncrights`); a user, a
    // trigger and the rest are the administrator's alone.
    if !call.administrator() {
        let detail = match kind {
            Kind::Function if call.may(18, old, ALTER) => None,
            Kind::Package if call.may(19, old, ALTER) => None,
            Kind::Table if call.may(1, old, ALTER) => None,
            Kind::View if call.may(6, old, ALTER) => None,
            Kind::Procedure if call.may(10, old, ALTER) => None,
            Kind::Link if call.may(12, old, ALTER) => None,
            Kind::Group if call.may(9, old, ALTER) => None,
            Kind::User if call.may(8, old, ALTER) => None,
            Kind::Publication if call.may(15, old, ALTER) => None,
            // A trigger is renamed by a user who may alter its table
            // (`fixtures/sql/ddrenamerights`).
            Kind::Trigger
                if dict
                    .object(object.parent)
                    .is_some_and(|table| call.may(1, &table.name, ALTER)) =>
            {
                None
            }
            // An integrity rule by a user who may alter both its tables; what
            // it then asks for is a property the original finds not set.
            Kind::Relation
                if [dd::relation::PRIMARY_TABLE, dd::relation::FOREIGN_TABLE]
                    .map(|slot| object.u32(slot).and_then(|id| dict.object(id)))
                    .iter()
                    .flatten()
                    .all(|table| call.may(1, &table.name, ALTER)) =>
            {
                return Err(ace_error::sql(
                    5138,
                    "Error 7200:  AQE Error:  State = HY000;   NativeError = 5138;  \
                     [SAP][Advantage SQL Engine][ASA] A request was made to get a property \
                     for an object in an Advantage Data Dictionary, but that property was \
                     not set for that object.",
                ));
            }
            Kind::Function => Some("User does not have permission to ALTER this function."),
            Kind::Package => Some("User does not have permission to ALTER this package."),
            _ => Some(""),
        };
        if let Some(detail) = detail {
            return Err(not_permitted(detail));
        }
    }
    if kind == Kind::Trigger && qualified {
        // The dictionary's own unique key refuses a trigger given the name a
        // sibling has, where the checks above did not see it.
        let sibling = dict
            .children(object.parent, Kind::Trigger)
            .into_iter()
            .any(|other| other.record() != object.record() && other.name.eq_ignore_ascii_case(new));
        if sibling {
            return Err(Error::UniqueViolation("de.ai:UNIQUETAG".to_owned()));
        }
    }
    let record = object.record();
    let id = object.id;
    let path = object.text(dd::table::RELATIVE_PATH);
    let files: Vec<(u32, String)> = match kind {
        Kind::Table => dict
            .children(id, Kind::IndexFile)
            .into_iter()
            .filter_map(|index| Some((index.record(), index.text(dd::index_file::RELATIVE_PATH)?)))
            .collect(),
        _ => Vec::new(),
    };
    drop(dict);
    let dbf = kind == Kind::Table
        && options == 0
        && path.as_deref().is_some_and(|path| {
            path.trim_end_matches('\0')
                .rsplit('.')
                .next()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dbf"))
        });
    if dbf {
        // The table is named last: a rename that stops part way leaves its
        // name as it was, and the files it moved recorded.
        let moved = rename_dbf_files(call, record, path.as_deref().unwrap_or(""), &files, new);
        if let Err(error) = moved {
            rebuild_dictionary(call.session, &call.conn, &call.add_path)
                .map_err(ace_error::sql_error)?;
            return Err(error);
        }
    }
    write_object_name(&call.add_path, record, new)?;
    if kind == Kind::Table && options == 0 && !dbf {
        rename_table_files(call, record, path.as_deref(), &files, new)?;
    }
    rebuild_dictionary(call.session, &call.conn, &call.add_path).map_err(ace_error::sql_error)
}

/// The memo file a DBF table's header says it has, named as the table is.
fn dbf_memo_file(table: &std::path::Path) -> Option<PathBuf> {
    use std::io::Read;
    let mut head = [0u8; 0x20];
    std::fs::File::open(table)
        .ok()?
        .read_exact(&mut head)
        .ok()?;
    let extension = match head[0] {
        0x83 | 0x87 => "dbt",
        0xF5 | 0xF7 => "fpt",
        0x30..=0x32 | 0xF8 | 0xF9 if head[0x1C] & 0x02 != 0 => "fpt",
        _ => return None,
    };
    let upper = table.extension().is_some_and(|e| e == "DBF");
    Some(table.with_extension(match upper {
        true => extension.to_ascii_uppercase(),
        false => extension.to_owned(),
    }))
}

/// The path `relative` has once its file is named `new`, its extension kept.
fn renamed_relative(relative: &str, new: &str) -> String {
    let plain = strip_relative(relative.trim_end_matches('\0'));
    let path = std::path::Path::new(plain);
    let mut target = path.with_file_name(new);
    if let Some(extension) = path.extension() {
        target.set_extension(extension);
    }
    target.to_string_lossy().into_owned()
}

/// A DBF table renamed takes its memo file and every index file of it with it
/// (a Clipper table's `.ntx` files are named for their orders, and each
/// becomes `<new>.ntx`, so that a second one finds the first there). The
/// table has to be openable exclusively, and neither its new name's files nor
/// its memo's may be there; then the index files go, one by one and each
/// recorded as it is moved, and the table last.
fn rename_dbf_files(
    call: &Call,
    record: u32,
    path: &str,
    index_files: &[(u32, String)],
    new: &str,
) -> Result<(), Error> {
    let folder = dictionary_folder(&call.add_path).to_path_buf();
    let table = under_folder(&folder, strip_relative(path.trim_end_matches('\0')));
    let memo = dbf_memo_file(&table);
    if crate::shared::in_use(&table)
        || !table.exists()
        || memo.as_ref().is_some_and(|memo| !memo.exists())
    {
        return Err(ace_error::sql(
            5067,
            "Unable to open the table exclusively.",
        ));
    }
    let target = under_folder(&folder, &renamed_relative(path, new));
    let memo_target = memo.as_ref().and_then(|memo| {
        let stem = target.file_stem()?;
        Some(memo.with_file_name(stem).with_extension(memo.extension()?))
    });
    for destination in std::iter::once(&target).chain(memo_target.as_ref()) {
        if destination.exists() {
            return Err(ace_error::sql(5020, "Unable to rename table, error: 183."));
        }
    }
    for (index, relative) in index_files {
        let name = renamed_relative(relative, new);
        let from = under_folder(&folder, strip_relative(relative.trim_end_matches('\0')));
        let to = under_folder(&folder, &name);
        if to.exists() {
            return Err(ace_error::sql(
                5020,
                &format!(
                    "Error moving table or index, destination file already exists: {}",
                    to.display()
                ),
            ));
        }
        crate::cache::evict_path(&from);
        std::fs::rename(&from, &to).map_err(|error| {
            ace_error::sql(
                5020,
                &format!(
                    "Error moving table or index, error code: {}.",
                    error.raw_os_error().unwrap_or(2)
                ),
            )
        })?;
        let mut stored = format!(".\\{name}\0").into_bytes();
        // An index file's path is stored a byte longer than it is: the byte
        // the old path has there, or none.
        let mut old = relative.trim_end_matches('\0').as_bytes().to_vec();
        old.push(0);
        stored.push(old.get(stored.len()).copied().unwrap_or(0));
        set_object_slot(
            call.session,
            &call.conn,
            &call.add_path,
            *index,
            dd::index_file::RELATIVE_PATH,
            &stored,
        )
        .map_err(ace_error::sql_error)?;
        write_object_name(&call.add_path, *index, &name)?;
    }
    crate::cache::evict_path(&table);
    let fail = |error: std::io::Error| {
        ace_error::sql(
            5020,
            &format!(
                "Unable to rename table, error: {}.",
                error.raw_os_error().unwrap_or(0)
            ),
        )
    };
    std::fs::rename(&table, &target).map_err(fail)?;
    if let (Some(memo), Some(memo_target)) = (memo, memo_target) {
        std::fs::rename(memo, memo_target).map_err(fail)?;
    }
    let stored = format!(".\\{}\0", renamed_relative(path, new)).into_bytes();
    set_object_slot(
        call.session,
        &call.conn,
        &call.add_path,
        record,
        dd::table::RELATIVE_PATH,
        &stored,
    )
    .map_err(ace_error::sql_error)
}

/// A table renamed takes its files with it: its own, and the index files that
/// are named as it was.
fn rename_table_files(
    call: &Call,
    record: u32,
    path: Option<&str>,
    index_files: &[(u32, String)],
    new: &str,
) -> Result<(), Error> {
    let folder = dictionary_folder(&call.add_path).to_path_buf();
    let renamed = |relative: &str| {
        let plain = strip_relative(relative.trim_end_matches('\0'));
        let path = std::path::Path::new(plain);
        let mut target = path.with_file_name(new);
        if let Some(extension) = path.extension() {
            target.set_extension(extension);
        }
        target.to_string_lossy().into_owned()
    };
    let stem_of = |relative: &str| {
        std::path::Path::new(strip_relative(relative.trim_end_matches('\0')))
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    };
    let Some(path) = path else {
        return Ok(());
    };
    let own = stem_of(path);
    let mut moves = vec![(record, dd::table::RELATIVE_PATH, path.to_owned(), None)];
    for (index, relative) in index_files {
        if stem_of(relative) == own {
            moves.push((
                *index,
                dd::index_file::RELATIVE_PATH,
                relative.clone(),
                Some(*index),
            ));
        }
    }
    for (slot_record, slot, relative, named) in moves {
        let target = renamed(&relative);
        let from = under_folder(&folder, strip_relative(relative.trim_end_matches('\0')));
        let to = under_folder(&folder, &target);
        if from.exists() {
            std::fs::rename(&from, &to).map_err(|_| Error::CorruptTable)?;
            // An ADT table's memo file goes with it.
            let memo = from.with_extension("adm");
            if named.is_none()
                && memo.exists()
                && from
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("adt"))
            {
                std::fs::rename(&memo, to.with_extension("adm"))
                    .map_err(|_| Error::CorruptTable)?;
            }
        }
        let mut stored = format!(".\\{target}\0").into_bytes();
        // An index file's path is stored a byte longer than it is: the byte
        // the old path has there, or none.
        if named.is_some() {
            let mut old = relative.trim_end_matches('\0').as_bytes().to_vec();
            old.push(0);
            stored.push(old.get(stored.len()).copied().unwrap_or(0));
        }
        set_object_slot(
            call.session,
            &call.conn,
            &call.add_path,
            slot_record,
            slot,
            &stored,
        )
        .map_err(ace_error::sql_error)?;
        // An index file object is named by its file.
        if let Some(index) = named {
            write_object_name(&call.add_path, index, &target)?;
        }
    }
    Ok(())
}

/// `sp_MoveDDObjectFile`: a table (with the index files named) or an index
/// file to another folder.
fn move_object_file(call: &Call) -> Result<(), Error> {
    let object_type = match call.integer(0) {
        Some(n @ (1 | 3)) => n as u16,
        _ => return Err(invalid_object_type("Parameter 2.")),
    };
    let options = call.integer(5).unwrap_or(0);
    if options != 0 {
        return Err(ace_error::sql(5019, "Parameter 7."));
    }
    let parent = call.text(4).unwrap_or("");
    if object_type == 1 && !parent.is_empty() {
        return Err(ace_error::sql(
            5021,
            "A parent cannot be specified for this object type.",
        ));
    }
    let name = call.text(1).unwrap_or("").trim();
    let listed = call.text(3).unwrap_or("");
    {
        let dict = call.dictionary()?;
        match object_type {
            1 => {
                let Some(table) = dict.named(Kind::Table, name) else {
                    return Err(not_valid(
                        "There is no such named table in the data dictionary.",
                    ));
                };
                // A name in the list that is none of the table's has no more said.
                let unknown = listed
                    .split(';')
                    .map(str::trim)
                    .filter(|file| !file.is_empty())
                    .any(|file| dict.child(table.id, Kind::IndexFile, file).is_none());
                if unknown {
                    return Err(ace_error::sql(5132, ""));
                }
            }
            _ => {
                if dict.named(Kind::IndexFile, name).is_none() {
                    return Err(not_valid(
                        "There is no such named index file in the data dictionary.",
                    ));
                }
                if dict.named(Kind::Table, parent.trim()).is_none() {
                    return Err(ace_error::sql(
                        5067,
                        "Cannot move index file while table is open.",
                    ));
                }
            }
        }
    }
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    // No folder is no file to move to.
    if call.text(2).unwrap_or("").trim().is_empty() {
        return Err(ace_error::sql(
            5020,
            "Error moving table or index, error code: 2.",
        ));
    }
    move_object_file_api(
        call.session,
        call.connection,
        object_type,
        name,
        call.text(2).unwrap_or("").trim(),
        listed,
        parent.trim(),
        0,
    )
    .map_err(table_error)
}

/// The properties `sp_ModifyTableProperty` knows by name.
const TABLE_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("USER_DEFINED_PROP", 3),
    ("TRIGGERS_DISABLED", 5),
    ("TABLE_VALIDATION_EXPR", 200),
    ("TABLE_VALIDATION_MSG", 201),
    ("TABLE_PRIMARY_KEY", 202),
    ("TABLE_AUTO_CREATE", 203),
    ("TABLE_TYPE", 204),
    ("TABLE_RELATIVE_PATH", 211),
    ("TABLE_DEFAULT_INDEX", 213),
    ("TABLE_ENCRYPTION", 214),
    ("TABLE_MEMO_BLOCK_SIZE", 215),
    ("TABLE_PERMISSION_LEVEL", 216),
    ("TABLE_CACHING", 219),
    ("TABLE_WEB_DELTA", 222),
    ("TABLE_CONCURRENCY_ENABLED", 223),
];

/// The properties `sp_ModifyFieldProperty` knows by name.
const FIELD_NAMES: &[(&str, u32)] = &[
    ("COMMENT", 1),
    ("USER_DEFINED_PROP", 3),
    ("FIELD_DEFAULT_VALUE", 300),
    ("FIELD_MIN_VALUE", 302),
    ("FIELD_MAX_VALUE", 303),
    ("FIELD_VALIDATION_MSG", 304),
    ("FIELD_TYPE", 306),
    ("FIELD_LENGTH", 307),
    ("FIELD_DECIMAL", 308),
    ("FIELD_NUM", 309),
    ("FIELD_OPTIONS", 310),
];

/// Whether a validation option is one that leaves the rows as they are: the
/// others check them, and a table with rows is not taken up here.
fn validation_option(option: &str) -> Option<bool> {
    match option {
        o if o.eq_ignore_ascii_case("NO_VALIDATE") => Some(false),
        o if o.eq_ignore_ascii_case("APPEND_FAIL") => Some(true),
        "0" => Some(false),
        "1" | "2" | "3" => Some(true),
        _ => None,
    }
}

/// Sets a text slot of the object at `record`, which an empty text clears.
fn set_text_slot(call: &Call, record: u32, slot: usize, value: &[u8]) -> Result<(), Error> {
    let clear = value.len() <= 1;
    write_object_slots(
        call.session,
        &call.conn,
        &call.add_path,
        record,
        &[(slot, (!clear).then_some(value))],
    )
    .map_err(ace_error::sql_error)
}

/// `sp_ModifyTableProperty`.
fn modify_table_property(call: &Call) -> Result<(), Error> {
    let change = change(call, 1, TABLE_NAMES)?;
    if change.id == 65535 {
        return Err(ace_error::sql(5131, ""));
    }
    let option = call.text(3).unwrap_or("");
    let name = call.text(0).unwrap_or("").trim();
    // The rows a validation is to check.
    let mut checks_rows = false;
    if change.id == 200 && change.value.len() > 1 {
        match validation_option(option) {
            Some(check) => checks_rows = check,
            None => {
                return Err(ace_error::sql(5019, "The validation option is not valid."));
            }
        }
    }
    let (record, table_id) = {
        let dict = call.dictionary()?;
        let table = dict
            .named(Kind::Table, name)
            .ok_or_else(|| not_valid("There is no such named table in the data dictionary."))?;
        (table.record(), table.id)
    };
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    let value = &change.value;
    let value_u16 = |detail: &str| -> Result<u16, Error> {
        match value.len() {
            2 => Ok(u16::from_le_bytes([value[0], value[1]])),
            _ => Err(buffer_size(detail)),
        }
    };
    let index_named = |text: &[u8]| -> Result<Option<u32>, Error> {
        let text = String::from_utf8_lossy(text.strip_suffix(&[0]).unwrap_or(text)).into_owned();
        if text.is_empty() {
            return Ok(None);
        }
        let dict = call.dictionary()?;
        match dict.child(table_id, Kind::Index, &text) {
            Some(index) => Ok(Some(index.id)),
            None => Err(not_valid(
                "The table does not have a key with the given name.",
            )),
        }
    };
    match change.id {
        1 => set_comment_of(call, record, value),
        3 => set_text_slot_verbatim(call, record, dd::table::USER_DEFINED, value),
        5 => match value.len() {
            0 => Err(buffer_size("The property is a 16 bit Boolean value.")),
            _ => set_object_presence(
                call.session,
                &call.conn,
                &call.add_path,
                record,
                dd::table::TRIGGERS_DISABLED,
                flag_on(value),
            )
            .map_err(ace_error::sql_error),
        },
        200 => {
            if value.len() > 1 {
                let text = String::from_utf8_lossy(&value[..value.len() - 1]).into_owned();
                let handle = table::open(
                    call.session,
                    call.connection,
                    name,
                    table::TableSettings::default(),
                );
                if let Ok(handle) = handle {
                    let compiled = table::get(call.session, handle).map(|t| t.compile(&text));
                    let _ = table::close(call.session, handle);
                    if matches!(compiled, Ok(Err(_))) {
                        return Err(ace_error::sql(
                            5160,
                            "The constraint is of the wrong data type or invalid syntax. Failed to set table record validation expression.",
                        ));
                    }
                }
                if checks_rows && table_has_rows(call, name) {
                    return Err(Error::NotImplemented);
                }
            }
            set_text_slot(call, record, dd::table::VALIDATION_EXPR, value)
        }
        201 => set_text_slot(call, record, dd::table::VALIDATION_MSG, value),
        202 => {
            let Some(key) = index_named(value)? else {
                return set_object_info1(call.session, &call.conn, &call.add_path, record, 0)
                    .map_err(ace_error::sql_error);
            };
            let unique = {
                let dict = call.dictionary()?;
                let order = dict.object(key).ok_or(Error::CorruptTable)?;
                let options = order
                    .slot(dd::index::OPTIONS)
                    .filter(|bytes| bytes.len() >= 4)
                    .map_or(0, |bytes| {
                        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                    });
                options & (0x01 | 0x800) != 0 && order.slot(dd::index::CONDITION).is_none()
            };
            if !unique {
                return Err(ace_error::sql(5134, "Primary key must be unique key."));
            }
            set_object_info1(call.session, &call.conn, &call.add_path, record, key as i32)
                .map_err(ace_error::sql_error)
        }
        203 => match value.len() {
            0 => Err(buffer_size("The property is a 16 bit Boolean value.")),
            _ => set_table_property_api(call.session, call.connection, name, 203, value)
                .map_err(ace_error::sql_error),
        },
        213 => {
            index_named(value)?;
            set_text_slot(call, record, dd::table::DEFAULT_INDEX, value)
        }
        214 => match value.len() {
            0 => Err(buffer_size("The property is a 16 bit Boolean value.")),
            _ => set_table_property_api(call.session, call.connection, name, 214, value)
                .map_err(ace_error::sql_error),
        },
        215 => {
            if value.is_empty() {
                return set_object_slot_cleared(call, record, dd::table::MEMO_BLOCK_SIZE);
            }
            let size = value_u16("The property is a 16 bit value.")?;
            if !(8..=32768).contains(&size) {
                return Err(ace_error::sql(5058, ""));
            }
            set_table_property_api(call.session, call.connection, name, 215, value)
                .map_err(ace_error::sql_error)
        }
        216 => {
            let level = value_u16("The property is a 16 bit value.")?;
            match level {
                1..=3 => set_table_property_api(call.session, call.connection, name, 216, value)
                    .map_err(ace_error::sql_error),
                _ => Err(ace_error::sql(
                    5127,
                    "The property value is not a valid table permission level.",
                )),
            }
        }
        219 => {
            let mode = value_u16("The property is a 16 bit value.")?;
            match mode {
                0..=2 => set_table_property_api(call.session, call.connection, name, 219, value)
                    .map_err(ace_error::sql_error),
                _ => Err(ace_error::sql(
                    5127,
                    "The property value is not a valid table caching mode.",
                )),
            }
        }
        222 | 223 => match value.len() {
            0 => Err(buffer_size("The property is a 16 bit Boolean value.")),
            _ if !flag_on(value) => {
                let bit = match change.id {
                    222 => dd::table::flags::WEB_DELTA,
                    _ => dd::table::flags::CONCURRENCY,
                };
                set_object_info2(
                    call.session,
                    &call.conn,
                    &call.add_path,
                    record,
                    bit as u32,
                    0,
                )
                .map_err(ace_error::sql_error)?;
                match change.id {
                    222 => marks_users(call),
                    _ => Ok(()),
                }
            }
            // Both need a row version column, which the tables here lack; a
            // dictionary that was asked for delta is of the version that has it.
            _ if change.id == 222 => {
                let database = call
                    .dictionary()?
                    .database()
                    .map(Object::record)
                    .ok_or(Error::CorruptTable)?;
                set_object_info1(call.session, &call.conn, &call.add_path, database, 4)
                    .map_err(ace_error::sql_error)?;
                Err(ace_error::sql(
                    5225,
                    "The table does not have a ROWVERSION column.",
                ))
            }
            _ => Err(ace_error::sql(
                5229,
                "A ROWVERSION field is required in order to enable concurrency control.",
            )),
        },
        _ => Err(cannot_be_set("table")),
    }
}

/// Whether the table `name` has any record.
fn table_has_rows(call: &Call, name: &str) -> bool {
    let Ok(handle) = table::open(
        call.session,
        call.connection,
        name,
        table::TableSettings::default(),
    ) else {
        return true;
    };
    let rows =
        table::get(call.session, handle).map_or(true, |t| t.record_count().map_or(true, |n| n > 0));
    let _ = table::close(call.session, handle);
    rows
}

/// Sets a text slot to the value as it is (a user-defined property).
fn set_text_slot_verbatim(
    call: &Call,
    record: u32,
    slot: usize,
    value: &[u8],
) -> Result<(), Error> {
    set_object_slot(
        call.session,
        &call.conn,
        &call.add_path,
        record,
        slot,
        value,
    )
    .map_err(ace_error::sql_error)
}

/// Takes a slot of the object at `record` away.
fn set_object_slot_cleared(call: &Call, record: u32, slot: usize) -> Result<(), Error> {
    set_object_presence(
        call.session,
        &call.conn,
        &call.add_path,
        record,
        slot,
        false,
    )
    .map_err(ace_error::sql_error)
}

/// `sp_ModifyFieldProperty`.
fn modify_field_property(call: &Call) -> Result<(), Error> {
    let option = call.text(4).unwrap_or("");
    let checks_rows = match validation_option(option) {
        Some(check) => check,
        None => {
            return Err(ace_error::sql(5019, "Parameter 7"));
        }
    };
    let change = change(call, 2, FIELD_NAMES)?;
    if change.id == 65535 {
        return Err(ace_error::sql(5021, "Table name"));
    }
    let table_name = call.text(0).unwrap_or("").trim();
    let field_name = call.text(1).unwrap_or("").trim();
    let table_id = call
        .dictionary()?
        .named(Kind::Table, table_name)
        .map(|t| t.id)
        .ok_or_else(|| not_valid("There is no such named table in the data dictionary."))?;
    if !call.administrator() {
        return Err(not_permitted(""));
    }
    let record = call
        .dictionary()?
        .child(table_id, Kind::Field, field_name)
        .map(Object::record)
        .ok_or_else(|| not_valid("The specified field is not part of the table."))?;
    let value = &change.value;
    match change.id {
        1 => set_comment_of(call, record, value),
        3 => set_text_slot_verbatim(call, record, dd::field::USER_DEFINED, value),
        300 | 302 | 303 => {
            // An empty value takes the slot away.
            if value.len() <= 1 {
                let slot = value_slot(Kind::Field, change.id);
                return set_text_slot(call, record, slot, value);
            }
            if checks_rows && table_has_rows(call, table_name) {
                return Err(Error::NotImplemented);
            }
            set_field_property_api(
                call.session,
                call.connection,
                table_name,
                field_name,
                change.id as u16,
                value,
            )
            .map_err(ace_error::sql_error)
        }
        301 => match value.len() {
            0 => Err(buffer_size("The property is a 16 bit Boolean value.")),
            _ => set_object_presence(
                call.session,
                &call.conn,
                &call.add_path,
                record,
                dd::field::NOT_NULL,
                !flag_on(value),
            )
            .map_err(ace_error::sql_error),
        },
        304 => set_text_slot(call, record, dd::field::VALIDATION_MSG, value),
        310 => Err(props::not_a_property()),
        _ => Err(ace_error::sql(
            5126,
            "The specified property cannot be set on a field object.",
        )),
    }
}

/// What the original engine does to every user's record when a table's web
/// delta is set off: the top bit of its `Info2` goes, and in a dictionary of
/// the version that has delta tables (4) another takes its place
/// (`fixtures/sql/spddtprops`).
fn marks_users(call: &Call) -> Result<(), Error> {
    let (version, users) = {
        let dict = call.dictionary()?;
        let version = dict.database().and_then(|d| d.info1).unwrap_or(2);
        let users: Vec<u32> = dict.all(Kind::User).iter().map(|u| u.record()).collect();
        (version, users)
    };
    let set = if version == 4 { 0x4000_0000 } else { 0 };
    for record in users {
        set_object_info2(
            call.session,
            &call.conn,
            &call.add_path,
            record,
            0x8000_0000,
            set,
        )
        .map_err(ace_error::sql_error)?;
    }
    Ok(())
}

/// `sp_SetDDEncryptionType`: what it refuses, then the conversion.
fn set_encryption_type(call: &Call) -> Result<(), Error> {
    let kind = call.text(3).unwrap_or("");
    if !["RC4", "AES128", "AES256"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(kind))
    {
        return Err(ace_error::sql(
            5019,
            "The encryption type must be RC4, AES128, or AES256",
        ));
    }
    if kind.to_ascii_uppercase().starts_with("AES") && call.text(2).unwrap_or("").is_empty() {
        return Err(ace_error::sql(
            5019,
            "The DDPassword parameter must be provided when converting to AES encryption",
        ));
    }
    let name = call.text(0).unwrap_or("");
    if name.is_empty() {
        return Err(ace_error::sql(5020, ""));
    }
    // A name is looked for under the dictionary's own path, and a table's
    // extension is put on one without.
    let mut path = match std::path::Path::new(name).is_absolute() {
        true => PathBuf::from(name),
        false => PathBuf::from(format!("{}/{name}", call.add_path.display())),
    };
    if path.extension().is_none() {
        path.set_extension("adt");
    }
    if !path.exists() {
        return Err(file_not_found(&path.display().to_string()));
    }
    let (admin, password) = (call.text(1).unwrap_or(""), call.text(2).unwrap_or(""));
    check_convertible(&path, admin, password)?;
    let target = match kind.to_ascii_uppercase().as_str() {
        "RC4" => crate::dictionary_write::convert::Target::Rc4,
        "AES128" => {
            crate::dictionary_write::convert::Target::Aes(tflip_storage::adt::ENCRYPTION_AES128)
        }
        _ => crate::dictionary_write::convert::Target::Aes(tflip_storage::adt::ENCRYPTION_AES256),
    };
    crate::dictionary_write::convert::convert_dictionary(
        call.session,
        &path,
        admin.as_bytes(),
        password.as_bytes(),
        target,
        call.integer(4).unwrap_or(0) != 0,
    )
}

/// What `sp_SetDDEncryptionType` refuses, before it converts, of the dictionary
/// at `path` (`fixtures/aes/dictwrite`, `24_convert_refused`): an AES
/// dictionary that encrypts its records, which no connection of the process
/// gave the password of (7068), or the password of another than the
/// `ddpassword` (7060); an `admin` password that is not its ADSSYS one (7078).
fn check_convertible(path: &std::path::Path, admin: &str, ddpassword: &str) -> Result<(), Error> {
    let dictionary = match Dictionary::open(path) {
        Ok(dictionary) => dictionary,
        Err(dd::Error::Encrypted) => {
            return Err(ace_error::sql(7068, &path.display().to_string()));
        }
        Err(_) => return Err(Error::CorruptTable),
    };
    let aes = dictionary.scheme() != tflip_storage::crypto::Scheme::Rc4;
    if aes && dictionary.encrypted() && dd::unlock(path, ddpassword.as_bytes()).is_err() {
        return Err(Error::Sql(SqlError::ace_bare(
            7060,
            "Invalid encryption password.  The specified password does not match the \
             password being used in the table.",
        )));
    }
    if dictionary.dictionary_key(admin.as_bytes()).is_none() {
        return Err(ace_error::sql(7078, "axServerConnect"));
    }
    Ok(())
}
