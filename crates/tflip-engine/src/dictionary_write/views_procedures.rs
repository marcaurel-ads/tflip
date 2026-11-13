//! Adding views and procedures to a dictionary, and removing procedures.

use super::*;

/// Adds a view to the data dictionary `connection` is to (`AdsDDAddView`):
/// checks its `SELECT` as the engine does (it parses, sorts only with a
/// `TOP`, reads tables and columns that exist and takes no parameters:
/// `fixtures/sql/ddviews.json`), writes a View object whose `STATEMENT` slot
/// holds the statement (from its first word to its last token) and whose
/// `STATEMENT_LENGTH` slot holds its length (the SQL and its NUL
/// terminator), stores `comment` in the dictionary memo file if it is not
/// empty, and rebuilds the dictionary index. A statement too long for the
/// record's property field (more than 268 bytes) goes on into the `More
/// Property` memo, as the original engine writes it
/// (`fixtures/dictionary/viewlong`).
pub fn add_view_api(
    session: &Session,
    connection: Handle,
    name: &str,
    sql: &str,
    comment: &str,
) -> Result<(), Error> {
    let conn = connection::get(session, connection)?;
    if !conn.dictionary {
        return Err(Error::InvalidConnection);
    }
    super::rights::require_create(&conn, 6, "view")?;
    let view = tflip_sql::check_view(name, sql).map_err(Error::Sql)?;
    crate::statement::check_select(session, connection, &view.select, view.params)?;
    if view.params > 0 {
        return Err(Error::Sql(SqlError::parameter_missing()));
    }
    add_view(session, connection, name, &view.text, comment)?;
    super::rights::grant_creator(session, connection, 6, name)
}

/// Writes the View object `name` for the checked statement `sql`, in the
/// slot of a deleted object or after the last.
pub(super) fn add_view(
    session: &Session,
    connection: Handle,
    name: &str,
    sql: &str,
    comment: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let clash = if dict.named(Kind::View, name).is_some() {
        Some("View already exists in the dictionary with the same name.")
    } else if dict.named(Kind::Table, name).is_some() {
        Some("Table exists in the dictionary with the same name.")
    } else {
        None
    };
    let procedure =
        dict.named(Kind::Procedure, name).is_some() || functions::function_or_package(&dict, name);
    drop(dict);
    if procedure {
        return Err(objects::name_taken(&add_path));
    }
    if name.is_empty() || clash.is_some() {
        return Err(ace_error::sql(
            5132,
            clash.unwrap_or("The object name is reserved or already in use."),
        ));
    }

    // The next object id is the dictionary's AutoInc next value (header 0x50).
    let next_id = read_next_id(&add_path)?;

    const DATABASE_ID: i32 = 1;
    let statement = format!("{sql}\0");
    let statement_length = (statement.len() as u16).to_le_bytes();
    let slots: &[(usize, &[u8])] = &[
        (dd::view::STATEMENT, statement.as_bytes()),
        (dd::view::STATEMENT_LENGTH, &statement_length),
    ];
    // A statement too long for the record's `Property` field goes on into
    // `More Property`, before the comment is stored.
    let memo_path = dd::memo_path(&add_path);
    let object = NewObject {
        id: next_id,
        parent: DATABASE_ID,
        kind: "View",
        name,
        info1: i32::MIN, // 0x80000000, as the engine writes it
        info2: 0x8000_0000,
        slots,
    };
    let mut record = dd::create::object_record_stepwise(
        &object,
        &memo_path,
        &[dd::view::STATEMENT, dd::view::STATEMENT_LENGTH],
    )
    .map_err(|_| Error::CorruptTable)?;
    dd::create::set_comment(&mut record, &memo_path, comment.as_bytes())
        .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// Adds a stored procedure to the data dictionary `connection` is to
/// (`AdsDDAddProcedure`): a StoredProc object under the Database, written as the
/// original engine writes it. For a SQL-script procedure (the `ADS_SCRIPT_PROC`
/// bit in `invoke_option`) `container` is the script, kept in the script slot;
/// otherwise `container` is the external library's name and `proc_name` the
/// function in it. The input and output parameter lists, the invoke option and
/// the comment (in the dictionary memo) are stored as the engine stores them. An
/// empty or already-used name fails with `AE_INVALID_OBJECT_NAME` (5132).
#[allow(clippy::too_many_arguments)]
pub fn add_procedure_api(
    session: &Session,
    connection: Handle,
    name: &str,
    container: &str,
    proc_name: &str,
    invoke_option: u32,
    in_params: &str,
    out_params: &str,
    comment: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::rights::require_create(&conn, 10, "stored procedure")?;
    if name.is_empty() {
        return Err(invalid_object_name(name));
    }
    let dict = open_dict(&add_path)?;
    if dict.named(Kind::Procedure, name).is_some() {
        return Err(invalid_object_name(name));
    }
    drop(dict);
    insert_procedure(
        session,
        &conn,
        &add_path,
        &Procedure {
            name,
            container,
            proc_name,
            invoke_option,
            in_params,
            out_params,
            comment,
        },
    )?;
    super::rights::grant_creator(session, connection, 10, name)
}

/// What a stored procedure's object keeps.
pub(super) struct Procedure<'a> {
    pub(super) name: &'a str,
    pub(super) container: &'a str,
    pub(super) proc_name: &'a str,
    pub(super) invoke_option: u32,
    pub(super) in_params: &'a str,
    pub(super) out_params: &'a str,
    pub(super) comment: &'a str,
}

/// Writes the StoredProc object of `procedure`, in the slot of a deleted
/// object or after the last, and rebuilds the index.
/// What the original engine refuses of a procedure's script, as it keeps it
/// (without its `BEGIN` and `END`, `fixtures/sql/limits_procs.json`): 65,534
/// bytes are a property too long, and more a script too large.
pub(crate) fn check_script_length(script: &str) -> Result<(), Error> {
    use crate::ace_error;
    match script.len() {
        65535.. => Err(ace_error::sql(
            5133,
            "The SQL script for the procedure is too large.",
        )),
        65534 => Err(ace_error::sql(5127, "The property is too long.")),
        _ => Ok(()),
    }
}

/// What it refuses of a function's script, which has no 5127 step: from 65,534
/// bytes a script too long, spelt with a full stop by `ALTER` alone
/// (`fixtures/sql/limits_procs.json`).
pub(crate) fn check_function_script_length(script: &str, alter: bool) -> Result<(), Error> {
    use crate::ace_error;
    match script.len() {
        65534.. => Err(ace_error::sql(
            5133,
            match alter {
                true => "The function script is too long.",
                false => "The function script is too long",
            },
        )),
        _ => Ok(()),
    }
}

/// What it refuses of a trigger's script: 65,534 bytes are a property too
/// long, the next size is kept, and more is more than the container takes
/// (`fixtures/sql/limits_procs.json`).
pub(crate) fn check_trigger_script_length(script: &str) -> Result<(), Error> {
    use crate::ace_error;
    match script.len() {
        65536.. => Err(ace_error::sql(
            5021,
            "Parameter 7 is too big. Container is limited to 64K.",
        )),
        65534 => Err(ace_error::sql(5127, "The property is too long.")),
        _ => Ok(()),
    }
}

pub(super) fn insert_procedure(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    procedure: &Procedure,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, create::NewObject};
    let Procedure {
        name,
        container,
        proc_name,
        invoke_option,
        in_params,
        out_params,
        comment,
    } = *procedure;
    const DATABASE_ID: i32 = 1;
    const ADS_SCRIPT_PROC: u32 = 0x4;
    let next_id = read_next_id(add_path)?;
    let is_script = invoke_option & ADS_SCRIPT_PROC != 0;
    // The engine stores each string NUL-terminated; an empty parameter list or
    // function name leaves its slot unset.
    let input = nul_bytes(in_params);
    let output = nul_bytes(out_params);
    let container_z = nul_bytes(container);
    let proc_z = nul_bytes(proc_name);
    let invoke = invoke_option.to_le_bytes();
    let mut slots: Vec<(usize, &[u8])> = Vec::new();
    if !in_params.is_empty() {
        slots.push((dd::procedure::INPUT, &input));
    }
    if !out_params.is_empty() {
        slots.push((dd::procedure::OUTPUT, &output));
    }
    if is_script {
        slots.push((dd::procedure::SCRIPT, &container_z));
    } else {
        slots.push((dd::procedure::DLL_NAME, &container_z));
        if !proc_name.is_empty() {
            slots.push((dd::procedure::DLL_FUNCTION_NAME, &proc_z));
        }
    }
    slots.push((dd::procedure::INVOKE_OPTION, &invoke));
    if is_script {
        check_script_length(container)?;
    }
    // The original sets the properties one at a time, so a long parameter list
    // or script moves in the memo file as the blob grows.
    let mut record = dd::create::object_record_stepwise(
        &NewObject {
            id: next_id,
            parent: DATABASE_ID,
            kind: "StoredProc",
            name,
            info1: i32::MIN, // 0x80000000, the engine's NULL Integer
            info2: 0x8000_0000,
            slots: &slots,
        },
        &dd::memo_path(add_path),
        &[
            dd::procedure::INPUT,
            dd::procedure::OUTPUT,
            dd::procedure::DLL_NAME,
            dd::procedure::DLL_FUNCTION_NAME,
            dd::procedure::SCRIPT,
            dd::procedure::INVOKE_OPTION,
        ],
    )
    .map_err(|_| Error::CorruptTable)?;
    dd::create::set_comment_strlen(&mut record, &dd::memo_path(add_path), comment.as_bytes())
        .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, conn, add_path)
}

/// Removes a stored procedure from the data dictionary `connection` is to
/// (`AdsDDRemoveProcedure`): soft-deletes its StoredProc object, frees its
/// comment memo, and rebuilds the index. A name that is no procedure fails with
/// `AE_INVALID_OBJECT_NAME` (5132).
pub fn remove_procedure_api(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let Some(proc) = dict.named(Kind::Procedure, name) else {
        return Err(not_valid("The Procedure is not part of the database."));
    };
    super::rights::require(
        &conn,
        10,
        name,
        super::rights::DROP,
        "Permission to drop the procedure is not granted.",
    )?;
    let record = proc.record();
    let mut slots = vec![record];
    slots.extend(permission_records(&dict, proc.id));
    drop(dict);

    delete_records(session, &conn, &add_path, &slots)
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn adds_a_view_to_a_dictionary_like_the_engine() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/new");
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddaddview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fixtures.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        add_view_api(&session, connect, "v", "SELECT * FROM t", "a comment").unwrap();

        let ours = std::fs::read(&add).unwrap();
        let golden = std::fs::read(fixtures.join("view_table.add")).unwrap();
        assert_eq!(ours.len(), golden.len(), "add length");
        // 5 records: Database, Table, two Fields and the View.
        assert_eq!(u32::from_le_bytes(ours[0x18..0x1c].try_into().unwrap()), 5);
        assert_eq!(
            u32::from_le_bytes(ours[0x50..0x54].try_into().unwrap()),
            6,
            "next id"
        );
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let rl = 524;
        // The View record (record 5) matches the engine byte for byte,
        // including its comment memo reference.
        assert_eq!(
            &ours[data + 4 * rl..data + 5 * rl],
            &golden[data + 4 * rl..data + 5 * rl],
            "the view record"
        );
        // The memo file (the comment went into it) and the index match.
        assert_eq!(
            std::fs::read(dir.join("db.am")).unwrap(),
            std::fs::read(fixtures.join("view_table.am")).unwrap(),
            "memo"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fixtures.join("view_table.ai")).unwrap(),
            "index"
        );

        // The underlying table is bound to the dictionary, so the engine
        // resolves the view's table.
        assert_eq!(
            std::fs::read(dir.join("t.adt")).unwrap(),
            std::fs::read(fixtures.join("t.adt")).unwrap(),
            "the table file is bound to the dictionary"
        );

        // The new view is visible on the connection.
        conn_has_view(&session, connect, "v");

        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Views whose statements do not fit the record's property field go on
    /// into the `More Property` memo as the original engine writes them
    /// (`fixtures/dictionary/viewlong`, made by `tools/ace/dd/ddwrite.c`): a
    /// statement that just fits (268 bytes), one a byte over, and longer ones,
    /// the longest with a comment after it in the memo file.
    #[test]
    fn adds_long_views_like_the_engine() {
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/viewlong");
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddlongview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        std::fs::copy(fixtures.join("free/t.adt"), dir.join("t.adt")).unwrap();
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        for (name, length, comment) in [
            ("v268", 268, ""),
            ("v269", 269, ""),
            ("v300", 300, ""),
            ("v1000", 1000, ""),
            ("v3000", 3000, "a comment"),
        ] {
            let head = "SELECT id FROM t WHERE s <> '";
            let sql = format!("{head}{}'", "x".repeat(length - head.len() - 1));
            add_view_api(&session, connect, name, &sql, comment).unwrap();
        }

        let ours = std::fs::read(&add).unwrap();
        let golden = std::fs::read(fixtures.join("db.add")).unwrap();
        assert_eq!(ours.len(), golden.len(), "add length");
        // Every record but the Database's (its login secret is random).
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let rl = 524;
        for record in 1..9 {
            let at = data + record * rl;
            assert_eq!(
                &ours[at..at + rl],
                &golden[at..at + rl],
                "record {}",
                record + 1
            );
        }
        for file in ["db.am", "db.ai", "t.adt"] {
            let golden = std::fs::read(fixtures.join(file)).unwrap();
            assert!(std::fs::read(dir.join(file)).unwrap() == golden, "{file}");
        }
        // The statements read back whole.
        let conn = connection::get(&session, connect).unwrap();
        let read = conn
            .with_database_opt(|db| db.view("v3000"))
            .flatten()
            .unwrap();
        assert_eq!(read.len(), 3000);

        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn adds_and_removes_procedures_like_the_engine() {
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
                    (mask_login(ours), mask_login(want))
                } else {
                    (ours, want)
                };
                assert_eq!(ours, want, "{prefix}: {file}");
            }
        };
        // Add: a SQL-script procedure (with in/out params and a comment) and an
        // external-library procedure — AdsDDAddProcedure.
        let add_two = |session: &Session, connect: Handle| {
            add_procedure_api(
                session,
                connect,
                "sp_a",
                " INSERT INTO __output VALUES (1); ",
                "",
                0x4,
                "id,Integer;name,CHAR,10;",
                "v,Integer;",
                "first proc",
            )
            .unwrap();
            add_procedure_api(
                session,
                connect,
                "sp_b",
                ".\\example.aep",
                "MyFunc",
                0x1,
                "a,Integer;",
                "b,Integer;",
                "",
            )
            .unwrap();
        };

        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddprocadd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        add_two(&session, connect);
        check(&dir, "procadd");
        // An empty or already-used name is refused, nothing written.
        for name in ["sp_a", "SP_A", ""] {
            assert!(matches!(
                add_procedure_api(
                    &session,
                    connect,
                    name,
                    " SELECT 1; ",
                    "",
                    0x4,
                    "a,Integer;",
                    "",
                    ""
                ),
                Err(Error::Sql(_))
            ));
        }
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);

        // Remove: add both, then remove the script procedure (freeing its comment
        // memo) — AdsDDRemoveProcedure.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddprocdel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        add_two(&session, connect);
        remove_procedure_api(&session, connect, "sp_a").unwrap();
        check(&dir, "procdel");
        assert!(matches!(
            remove_procedure_api(&session, connect, "nope"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
