//! Views, procedures and databases made and dropped with SQL, as the
//! original engine does (`fixtures/sql/ddviews`, `ddprocs`): the same
//! objects the `AdsDD*` calls write, with the SQL engine's own refusals.

use std::path::Path;

use tflip_sql::SqlError;
use tflip_sql::ast::{CreateDatabase, CreateProcedure, CreateView, Named, ProcBody, ProcParam};
use tflip_storage::create::FieldSpec;
use tflip_storage::dd::{self, Kind};
use tflip_storage::field::FieldType;

use super::rights::{ALTER, DROP, grant_creator, may, may_create, refuse};
use super::{
    Procedure, add_view, bump_next_id, dictionary_connection, insert_procedure, not_valid,
    nul_bytes, open_dict, remove_procedure_api, remove_view_api, under_folder, write_dictionary,
    write_object_slots,
};
use crate::ace_error;
use crate::handle::Handle;
use crate::{Error, Session, connection};

/// The refusal of a name another kind of object has. The original has
/// started the object by then: it takes the next object id, and a slot of
/// the dictionary that it leaves blank and free (`fixtures/sql/ddprocs`).
pub(crate) fn name_taken(add_path: &Path) -> Error {
    match bump_next_id(add_path, true) {
        Ok(()) => not_valid("An object with the same name already exists in the data dictionary."),
        Err(error) => error,
    }
}

/// The `ADS_DD_*_OBJECT` types of what the statements make.
const VIEW: u16 = 6;
const PROCEDURE: u16 = 10;

/// The connection's dictionary, or the refusal of a connection to none.
fn dictionary(session: &Session, connection: Handle) -> Result<(), Error> {
    match connection::get(session, connection)?.dictionary {
        true => Ok(()),
        false => Err(Error::Sql(SqlError::no_dictionary())),
    }
}

/// `CREATE VIEW`: its `SELECT` is checked before the connection is, so
/// that a free connection says first what is wrong with the statement.
pub(crate) fn create_view(
    session: &Session,
    connection: Handle,
    create: &CreateView,
    params: usize,
) -> Result<(), Error> {
    // Whatever the user's rights on the tables it reads (`fixtures/sql/ddrights`).
    let check = || crate::statement::check_select(session, connection, &create.select, params);
    match connection::get(session, connection) {
        Ok(owner) => owner.as_owner(check),
        Err(_) => check(),
    }?;
    if params > 0 {
        return Err(Error::Sql(SqlError::parameter_missing()));
    }
    dictionary(session, connection)?;
    let (conn, _) = dictionary_connection(session, connection)?;
    if !may_create(&conn, VIEW) {
        return Err(refuse("Cannot create view object in the data dictionary."));
    }
    add_view(session, connection, &create.name.name, &create.text, "")?;
    grant_creator(session, connection, VIEW, &create.name.name)
}

/// `DROP VIEW`.
pub(crate) fn drop_view(session: &Session, connection: Handle, name: &Named) -> Result<(), Error> {
    dictionary(session, connection)?;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let found = dict.named(Kind::View, &name.name).is_some();
    drop(dict);
    if !found {
        return Err(not_valid("The view is not part of the database."));
    }
    if !may(&conn, VIEW, &name.name, DROP) {
        return Err(refuse("Permission to drop the view is not granted."));
    }
    remove_view_api(session, connection, &name.name)
}

/// How the dictionary spells a parameter's type (`fixtures/sql/ddprocs`).
fn definition(param: &ProcParam, spec: &FieldSpec) -> Result<String, Error> {
    use FieldType::*;
    let name = &spec.name;
    let (length, decimals) = (spec.length, spec.decimals);
    Ok(match spec.field_type {
        ShortInt => format!("{name},SHORTINT;"),
        Integer => format!("{name},Integer;"),
        LongInt => format!("{name},LONGINTEGER;"),
        Double => format!("{name},DOUBLE,{decimals};"),
        Numeric => format!("{name},Numeric,{length},{decimals};"),
        Logical => format!("{name},LOGICAL;"),
        Date => format!("{name},DATE;"),
        TimeStamp => format!("{name},TIMESTAMP;"),
        Time => format!("{name},TIME;"),
        VarChar => format!("{name},VARCHARFOX,{length};"),
        Character => format!("{name},CHAR,{length};"),
        CiCharacter => format!("{name},CICHAR,{length};"),
        Memo => format!("{name},Memo;"),
        Binary => format!("{name},BINARY;"),
        Money => format!("{name},Money;"),
        Raw => format!("{name},RAW,{length};"),
        VarBinary => format!("{name},VARBINARYFOX,{length};"),
        CurDouble => format!("{name},CURDOUBLE,2;"),
        NChar => format!("{name},NCHAR,{length};"),
        NVarChar => format!("{name},NVARCHAR,{length};"),
        NMemo => format!("{name},NMEMO;"),
        RowVersion => format!("{name},Rowversion;"),
        ModTime => format!("{name},MODTIME;"),
        Guid => format!("{name},GUID;"),
        // An input table has an auto-increment field of its own.
        AutoInc if !param.output => {
            return Err(ace_error::sql(
                5012,
                &format!("Only one auto-increment field allowed per table. Field: \"{name}\""),
            ));
        }
        AutoInc => format!("{name},AutoInc;"),
        _ => format!("{name},{};", param.column.type_name.to_ascii_uppercase()),
    })
}

/// The path of a library as the dictionary keeps it: relative to the
/// dictionary's folder, with backslashes.
fn library_path(library: &str) -> Result<String, Error> {
    let path = library.replace('/', "\\");
    let bad = |why: &str| ace_error::sql(5020, why);
    if path.starts_with("\\\\") {
        return Err(bad("The path cannot be on a different server."));
    }
    if path.as_bytes().get(1) == Some(&b':') {
        return Err(bad(
            "The supplied path cannot be converted to relative path.",
        ));
    }
    Ok(if path.starts_with(".\\") || path.starts_with("..\\") {
        path
    } else if path.starts_with('\\') {
        // The original stores a path with a leading backslash this way.
        format!("......{path}")
    } else {
        format!(".\\{path}")
    })
}

/// A procedure's properties as `CREATE PROCEDURE` says them.
struct Properties {
    input: String,
    output: String,
    /// The script, or the library.
    container: String,
    function: String,
    invoke: u32,
}

/// The properties of the procedure `create` defines, its parameters
/// checked as the fields of a table are.
fn properties(create: &CreateProcedure) -> Result<Properties, Error> {
    let (mut input, mut output) = (String::new(), String::new());
    for param in &create.params {
        let spec = crate::ddl::field_spec(&param.column)?;
        let text = definition(param, &spec)?;
        match param.output {
            true => output.push_str(&text),
            false => input.push_str(&text),
        }
    }
    let varying = if create.varying { 0x1000 } else { 0 };
    let (container, function, invoke) = match &create.body {
        ProcBody::Script(script) => (script.clone(), String::new(), 0x4 | varying),
        ProcBody::External {
            function,
            library,
            com: false,
        } => (library_path(library)?, function.clone(), 0x1),
        ProcBody::External {
            function,
            library,
            com: true,
        } => (library.clone(), function.clone(), 0x2),
    };
    Ok(Properties {
        input,
        output,
        container,
        function,
        invoke,
    })
}

/// `CREATE PROCEDURE` and `ALTER PROCEDURE`, which writes the new
/// properties into the object, so that its permissions stay.
pub(crate) fn create_procedure(
    session: &Session,
    connection: Handle,
    create: &CreateProcedure,
) -> Result<(), Error> {
    let properties = properties(create)?;
    dictionary(session, connection)?;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if !create.alter && !may_create(&conn, PROCEDURE) {
        return Err(refuse(
            "Cannot create stored procedure object in the data dictionary.",
        ));
    }
    let name = &create.name.name;
    let dict = open_dict(&add_path)?;
    let existing = dict.named(Kind::Procedure, name).map(|p| p.record());
    let other = [Kind::Table, Kind::View]
        .into_iter()
        .any(|kind| dict.named(kind, name).is_some())
        || super::functions::function_or_package(&dict, name);
    drop(dict);
    if create.alter {
        let Some(record) = existing else {
            return Err(not_valid(
                "There is no such named stored procedure in the data dictionary.",
            ));
        };
        if !may(&conn, PROCEDURE, name, ALTER) {
            return Err(refuse(""));
        }
        let script = matches!(create.body, ProcBody::Script(_));
        if script {
            super::views_procedures::check_script_length(&properties.container)?;
        }
        let text = |s: &str| (!s.is_empty()).then(|| nul_bytes(s));
        let (dll, function, script_text) = match script {
            true => (None, None, Some(nul_bytes(&properties.container))),
            false => (
                Some(nul_bytes(&properties.container)),
                text(&properties.function),
                None,
            ),
        };
        let (input, output) = (text(&properties.input), text(&properties.output));
        let invoke = properties.invoke.to_le_bytes();
        return write_object_slots(
            session,
            &conn,
            &add_path,
            record,
            &[
                (dd::procedure::INPUT, input.as_deref()),
                (dd::procedure::OUTPUT, output.as_deref()),
                (dd::procedure::DLL_NAME, dll.as_deref()),
                (dd::procedure::DLL_FUNCTION_NAME, function.as_deref()),
                (dd::procedure::INVOKE_OPTION, Some(&invoke)),
                (dd::procedure::SCRIPT, script_text.as_deref()),
            ],
        );
    }
    if existing.is_some() {
        return Err(not_valid(
            "Procedure already exists in the dictionary with the same name.",
        ));
    }
    if other {
        return Err(name_taken(&add_path));
    }
    insert_procedure(
        session,
        &conn,
        &add_path,
        &Procedure {
            name,
            container: &properties.container,
            proc_name: &properties.function,
            invoke_option: properties.invoke,
            in_params: &properties.input,
            out_params: &properties.output,
            comment: "",
        },
    )?;
    grant_creator(session, connection, PROCEDURE, name)
}

/// `DROP PROCEDURE`.
pub(crate) fn drop_procedure(
    session: &Session,
    connection: Handle,
    name: &Named,
) -> Result<(), Error> {
    dictionary(session, connection)?;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let found = dict.named(Kind::Procedure, &name.name).is_some();
    drop(dict);
    if !found {
        return Err(not_valid("The Procedure is not part of the database."));
    }
    if !may(&conn, PROCEDURE, &name.name, DROP) {
        return Err(refuse("Permission to drop the procedure is not granted."));
    }
    remove_procedure_api(session, connection, &name.name)
}

/// `CREATE DATABASE`: a dictionary in the folder of the connection (a
/// dictionary connection's too), with the extension `.add` in place of any
/// other (`fixtures/sql/dddatabase`).
pub(crate) fn create_database(
    session: &Session,
    connection: Handle,
    create: &CreateDatabase,
) -> Result<(), Error> {
    let owner = connection::get(session, connection)?;
    let mut add = match Path::new(&create.name).is_absolute() {
        true => Path::new(&create.name).to_path_buf(),
        false => under_folder(&owner.folder(), &create.name),
    };
    add.set_extension("add");
    if add.exists() {
        return Err(ace_error::sql(5128, ""));
    }
    let aes = create
        .aes
        .as_ref()
        .map(|(kind, password)| super::tables::NewAes {
            kind: *kind,
            password,
            encrypt: create.encrypt,
        });
    // Without an AES type, ENCRYPT TRUE enciphers with RC4 whatever else is
    // given (`fixtures/aes/dictwrite`, `41_rc4_sql`).
    let rc4 = aes.is_none() && create.encrypt;
    write_dictionary(session, &add, &create.description, aes, rc4).map_err(|error| match error {
        Error::FileCreate => ace_error::sql(7040, ""),
        error => error,
    })
}
