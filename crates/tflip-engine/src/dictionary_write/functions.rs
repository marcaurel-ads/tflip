//! Functions and packages made, changed and dropped with SQL, as the
//! original engine does (`fixtures/sql/ddfuncs`): no `AdsDD*` call makes
//! them.

use std::path::Path;

use tflip_sql::SqlError;
use tflip_sql::ast::{CreateFunction, FunctionDdl, Named};
use tflip_storage::dd::{self, Dictionary, Kind, Object};

use super::rights::{ALTER, DROP, grant_creator, may, may_create, refuse};
use super::{
    bump_next_id, delete_records, dictionary_connection, nul_bytes, open_dict,
    open_dictionary_file, permission_records, read_next_id, rebuild_dictionary, write_object_name,
    write_object_slots,
};
use crate::dictionary_write::not_valid;
use crate::handle::Handle;
use crate::{Error, Session, connection};

/// The Database object, the parent of the functions that no package owns.
const DATABASE_ID: u32 = 1;

/// A statement on the functions and packages of the connection's dictionary.
pub(crate) fn function_ddl(
    session: &Session,
    connection: Handle,
    ddl: &FunctionDdl,
) -> Result<(), Error> {
    if !connection::get(session, connection)?.dictionary {
        return Err(Error::Sql(SqlError::no_dictionary()));
    }
    match ddl {
        FunctionDdl::Create(create) => create_function(session, connection, create),
        FunctionDdl::Drop { package, name } => {
            drop_function(session, connection, package.as_ref(), name)
        }
        FunctionDdl::CreatePackage { name, description } => {
            create_package(session, connection, name, description.as_deref())
        }
        FunctionDdl::DropPackage(name) => drop_package(session, connection, name),
    }
}

/// The package `package` names, or 5132.
fn package<'a>(dict: &'a Dictionary, package: &Named) -> Result<&'a Object, Error> {
    dict.named(Kind::Package, &package.name)
        .ok_or_else(|| not_valid("There is no such named package in the data dictionary."))
}

/// The object the function `name` of the package `package_name` is, if it
/// exists: in the package, or in no package.
fn find_function<'a>(
    dict: &'a Dictionary,
    package_name: Option<&Named>,
    name: &Named,
) -> Result<&'a Object, Error> {
    let parent = match package_name {
        Some(named) => package(dict, named)?.id,
        None => DATABASE_ID,
    };
    dict.child(parent, Kind::Function, &name.name)
        .ok_or_else(|| not_valid("There is no such function in the database."))
}

/// Whether the name `name` is taken in the data dictionary by an object that
/// no package owns.
pub(crate) fn database_name_taken(dict: &Dictionary, name: &str) -> bool {
    [
        Kind::Table,
        Kind::View,
        Kind::Procedure,
        Kind::Package,
        Kind::Group,
        Kind::User,
    ]
    .into_iter()
    .any(|kind| dict.named(kind, name).is_some())
        || dict.child(DATABASE_ID, Kind::Function, name).is_some()
}

/// Whether the name `name` is a package's, or a function's that no package
/// owns: the objects that a table, view or procedure may not be named after.
pub(crate) fn function_or_package(dict: &Dictionary, name: &str) -> bool {
    dict.named(Kind::Package, name).is_some()
        || dict.child(DATABASE_ID, Kind::Function, name).is_some()
}

/// `CREATE FUNCTION` and `ALTER FUNCTION`.
fn create_function(
    session: &Session,
    connection: Handle,
    create: &CreateFunction,
) -> Result<(), Error> {
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if !create.alter && !may_create(&conn, 18) {
        return Err(refuse(
            "Cannot create function object in the data dictionary.",
        ));
    }
    let dict = open_dict(&add_path)?;
    let name = &create.name;
    let description = create.description.as_deref().unwrap_or("");
    if create.alter {
        let found = find_function(&dict, create.package.as_ref(), name)?;
        let record = found.record();
        match &create.package {
            Some(package) if !may(&conn, 19, &package.name, ALTER) => {
                return Err(refuse(
                    "User does not have permission to ALTER function in this package.",
                ));
            }
            None if !may(&conn, 18, &name.name, ALTER) => {
                return Err(refuse(
                    "User does not have permission to ALTER this function.",
                ));
            }
            _ => {}
        }
        if let Some(new) = &create.new_name {
            refuse_rename(&dict, found, &new.name, &add_path)?;
        }
        super::views_procedures::check_function_script_length(&create.body, true)?;
        drop(dict);
        let script = nul_bytes(&create.body);
        let return_type = nul_bytes(&create.returns);
        // An altered function with no parameters keeps an empty list.
        let params = nul_bytes(&create.params);
        write_object_slots(
            session,
            &conn,
            &add_path,
            record,
            &[
                (dd::function::RETURN_TYPE, Some(&return_type)),
                (dd::function::PARAMETERS, Some(&params)),
                (dd::function::SCRIPT, Some(&script)),
            ],
        )?;
        {
            let mut file = open_dictionary_file(&add_path)?;
            file.write_part(
                record,
                dd::create::INFO2,
                &(create.param_count as u32).to_le_bytes(),
            )
            .map_err(|_| Error::CorruptTable)?;
        }
        if let Some(new) = &create.new_name {
            write_object_name(&add_path, record, &new.name)?;
        }
        replace_description(session, &conn, &add_path, record, description)?;
        return Ok(());
    }
    let parent = match &create.package {
        Some(named) => {
            let package = package(&dict, named)?;
            if !may(&conn, 19, &package.name, ALTER) {
                return Err(refuse(
                    "ALTER permission to the package is needed to create function in the package.",
                ));
            }
            if dict.child(package.id, Kind::Function, &name.name).is_some() {
                drop(dict);
                bump_next_id(&add_path, false)?;
                return Err(not_valid(
                    "A function with the same name already exists in the package.",
                ));
            }
            package.id
        }
        None => {
            if database_name_taken(&dict, &name.name) {
                drop(dict);
                bump_next_id(&add_path, false)?;
                return Err(not_valid(
                    "An object with the same name already exists in the data dictionary.",
                ));
            }
            DATABASE_ID
        }
    };
    drop(dict);
    super::views_procedures::check_function_script_length(&create.body, false)?;
    let next_id = read_next_id(&add_path)?;
    let return_type = nul_bytes(&create.returns);
    let params = nul_bytes(&create.params);
    let script = nul_bytes(&create.body);
    let mut slots: Vec<(usize, &[u8])> = vec![(dd::function::RETURN_TYPE, &return_type)];
    if !create.params.is_empty() {
        slots.push((dd::function::PARAMETERS, &params));
    }
    slots.push((dd::function::SCRIPT, &script));
    insert(
        session,
        &conn,
        &add_path,
        dd::create::NewObject {
            id: next_id,
            parent: parent as i32,
            kind: "Function",
            name: &name.name,
            info1: i32::MIN,
            info2: create.param_count as u32,
            slots: &slots,
        },
        description,
    )?;
    match create.package {
        Some(_) => Ok(()),
        None => grant_creator(session, connection, 18, &name.name),
    }
}

/// Gives the object at `record` the description `description`, which is
/// written over the one it has when it takes as many memo blocks, else
/// stored anew with the old one freed after it. An empty one takes the
/// description away but leaves its memo block taken.
fn replace_description(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    record: u32,
    description: &str,
) -> Result<(), Error> {
    let memo = dd::memo_path(add_path);
    let (block, length) =
        dd::create::comment_ref(add_path, record).map_err(|_| Error::CorruptTable)?;
    let corrupt = |_| Error::CorruptTable;
    let at = dd::create::COMMENT;
    let mut file = open_dictionary_file(add_path)?;
    if description.is_empty() {
        file.write_part(record, at, &[0u8; 8])
            .map_err(|_| Error::CorruptTable)?;
        drop(file);
        return rebuild_dictionary(session, conn, add_path);
    }
    match dd::create::rewrite_comment_strlen(&memo, block, length, description.as_bytes())
        .map_err(corrupt)?
    {
        Some(stored) => file
            .write_part(record, at + 4, &stored.to_le_bytes())
            .map_err(|_| Error::CorruptTable)?,
        None => {
            let mut field = vec![0u8; dd::create::RECORD_LENGTH];
            dd::create::set_comment_strlen(&mut field, &memo, description.as_bytes())
                .map_err(corrupt)?;
            file.write_part(record, at, &field[at..at + 8])
                .map_err(|_| Error::CorruptTable)?;
            dd::create::free_comment(&memo, block, length).map_err(corrupt)?;
        }
    }
    drop(file);
    rebuild_dictionary(session, conn, add_path)
}

/// The refusal of a function's new name another object has: the key of the
/// object's name in the dictionary's index that is not unique.
fn refuse_rename(
    dict: &Dictionary,
    function: &Object,
    new_name: &str,
    add_path: &Path,
) -> Result<(), Error> {
    let other = |o: &&Object| o.record() != function.record();
    let table = add_path.with_extension("ai");
    let table = table
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let same_parent = dict
        .children(function.parent, Kind::Function)
        .into_iter()
        .filter(other)
        .any(|o| o.name.eq_ignore_ascii_case(new_name));
    if same_parent {
        return Err(Error::Sql(SqlError::not_unique(&table, "UNIQUETAG")));
    }
    let in_database = function.parent == DATABASE_ID
        && [
            Kind::Table,
            Kind::View,
            Kind::Procedure,
            Kind::Package,
            Kind::Group,
            Kind::User,
        ]
        .into_iter()
        .any(|kind| dict.named(kind, new_name).is_some());
    if in_database {
        return Err(Error::Sql(SqlError::not_unique(&table, "DBOBJECTNAMES")));
    }
    Ok(())
}

/// Writes the object `object` with its description, in the slot of a deleted
/// object or after the last.
fn insert(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    object: dd::create::NewObject,
    description: &str,
) -> Result<(), Error> {
    let next_id = object.id + 1;
    let memo = dd::memo_path(add_path);
    // The original sets a function's properties one at a time: the parameters,
    // then the return type, then the script (`fixtures/sql/spddlong`).
    let order = [
        dd::function::PARAMETERS,
        dd::function::RETURN_TYPE,
        dd::function::SCRIPT,
    ];
    let mut record = dd::create::object_record_stepwise(&object, &memo, &order)
        .map_err(|_| Error::CorruptTable)?;
    dd::create::set_comment_strlen(&mut record, &memo, description.as_bytes())
        .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(add_path, &[record], next_id).map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, conn, add_path)
}

/// `DROP FUNCTION`.
fn drop_function(
    session: &Session,
    connection: Handle,
    package_name: Option<&Named>,
    name: &Named,
) -> Result<(), Error> {
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let function = find_function(&dict, package_name, name)?;
    match package_name {
        Some(package) if !may(&conn, 19, &package.name, ALTER) => {
            return Err(refuse(
                "ALTER permission to the package is needed to drop a function in the package.",
            ));
        }
        None if !may(&conn, 18, &name.name, DROP) => {
            return Err(refuse(
                "DROP permission to the function required to drop this function.",
            ));
        }
        _ => {}
    }
    let mut slots = vec![function.record()];
    slots.extend(permission_records(&dict, function.id));
    drop(dict);
    delete_records(session, &conn, &add_path, &slots)
}

/// `CREATE PACKAGE`.
fn create_package(
    session: &Session,
    connection: Handle,
    name: &Named,
    description: Option<&str>,
) -> Result<(), Error> {
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if !may_create(&conn, 19) {
        return Err(refuse(
            "Cannot create package object in the data dictionary.",
        ));
    }
    let dict = open_dict(&add_path)?;
    let taken = database_name_taken(&dict, &name.name);
    drop(dict);
    if taken {
        return Err(super::name_taken(&add_path));
    }
    let next_id = read_next_id(&add_path)?;
    insert(
        session,
        &conn,
        &add_path,
        dd::create::NewObject {
            id: next_id,
            parent: DATABASE_ID as i32,
            kind: "Package",
            name: &name.name,
            info1: i32::MIN,
            info2: 0x8000_0000,
            slots: &[],
        },
        description.unwrap_or(""),
    )?;
    grant_creator(session, connection, 19, &name.name)
}

/// `DROP PACKAGE`, which takes its functions with it.
fn drop_package(session: &Session, connection: Handle, name: &Named) -> Result<(), Error> {
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let package = package(&dict, name)?;
    if !may(&conn, 19, &package.name, DROP) {
        return Err(refuse(""));
    }
    let functions: Vec<u32> = dict
        .children(package.id, Kind::Function)
        .iter()
        .map(|f| f.record())
        .collect();
    // The package goes first, then the rights on it, then its functions.
    let mut slots = vec![package.record()];
    slots.extend(permission_records(&dict, package.id));
    slots.extend(functions);
    drop(dict);
    delete_records(session, &conn, &add_path, &slots)
}
