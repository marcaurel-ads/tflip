//! The system procedures that report on the server and the connection
//! (`sp_mgGetActivityInfo`, `sp_mgGetAllTables`, `sp_mgGetConfigInfo`, ...),
//! and set what a connection does (`sp_SetStatementLimit`,
//! `sp_SetRequestPriority`, `sp_mgKillUser`, `sp_mgResetCommStats`). They
//! answer as the original's local server does (`fixtures/sql/spm.json`,
//! `spm1.json`): tflip's session is its "server", and the one OS user its
//! only user.

use std::sync::OnceLock;
use std::time::Instant;

use tflip_sql::ast::Execute;
use tflip_sql::{SqlError, V};
use tflip_storage::build::Column;
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use super::Results;
use super::args::{Kind, Spec, check_arguments, column};
use crate::ace_error;
use crate::handle::{self, Handle};
use crate::table::{self, TableType};
use crate::{Error, Session, connection};

const NONE: &[(&str, Kind)] = &[];
const TABLE: (&str, Kind) = ("TableName", Kind::Text);
const USER: (&str, Kind) = ("UserName", Kind::Text);

const fn spec(
    name: &'static str,
    params: &'static [(&'static str, Kind)],
    required: usize,
    results: bool,
) -> Spec {
    Spec {
        name,
        params,
        required,
        results,
    }
}

const SPECS: [Spec; 30] = [
    spec(
        "SP_SETSTATEMENTLIMIT",
        &[("Limit", Kind::Integer)],
        1,
        false,
    ),
    spec(
        "SP_SETREQUESTPRIORITY",
        &[("Priority", Kind::Memo)],
        1,
        false,
    ),
    spec(
        "SP_CANCELQUERY",
        &[("Query Number", Kind::Integer)],
        1,
        false,
    ),
    spec("SP_MGGETSERVERTYPE", NONE, 0, true),
    spec("SP_MGGETACTIVITYINFO", NONE, 0, true),
    spec("SP_MGGETCOMMSTATS", NONE, 0, true),
    spec("SP_MGRESETCOMMSTATS", NONE, 0, false),
    spec("SP_MGGETCONFIGINFO", NONE, 0, true),
    spec("SP_MGGETCONFIGMEMORY", NONE, 0, true),
    spec("SP_MGGETCONFIGMEMORY90", NONE, 0, true),
    spec("SP_MGGETCACHEINFO", NONE, 0, true),
    spec("SP_MGGETTABLECACHEINFO", &[TABLE], 1, true),
    spec("SP_MGGETUSAGEINFO", NONE, 0, true),
    spec("SP_MGGETCONNECTEDUSERS", NONE, 0, true),
    spec("SP_MGGETALLTABLES", NONE, 0, true),
    spec("SP_MGGETALLINDEXES", NONE, 0, true),
    spec("SP_MGGETWORKERTHREADACTIVITY", NONE, 0, true),
    spec("SP_MGGETALLLOCKS", &[TABLE], 1, true),
    spec("SP_MGGETINDEXUSERS", &[("IndexName", Kind::Text)], 1, true),
    spec(
        "SP_MGGETLOCKOWNER",
        &[TABLE, ("RecordNumber", Kind::Integer)],
        2,
        true,
    ),
    spec("SP_MGGETTABLEINDEXES", &[TABLE], 1, true),
    spec("SP_MGGETTABLEUSERS", &[TABLE], 1, true),
    spec("SP_MGGETUSERINDEXES", &[USER], 1, true),
    spec("SP_MGGETUSERLOCKS", &[TABLE, USER], 2, true),
    spec("SP_MGGETUSERTABLES", &[USER], 1, true),
    spec("SP_MGKILLUSER", &[USER], 1, false),
    spec(
        "SP_MGKILLUSER90",
        &[USER, ("Property", Kind::Integer), ("Value", Kind::Memo)],
        3,
        false,
    ),
    // The ones only the root dictionary's administrator may run.
    spec("SP_MGGETCRASHDUMPINFO", NONE, 0, true),
    spec("SP_MGGETERRORLOG", NONE, 0, true),
    spec("SP_MGSETCONFIGVALUE", NONE, 0, true),
];

fn find(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// Whether `name` is one of these procedures.
pub(crate) fn handles(name: &str) -> bool {
    find(name).is_some()
}

/// Whether the procedure `name` has a result set (true for any that is not
/// one of these).
pub(crate) fn has_results(name: &str) -> bool {
    find(name).is_none_or(|s| s.results)
}

/// The parameters of the procedure `name`, as fields (none for one that is
/// not one of these).
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    find(name).map_or_else(Vec::new, super::args::inputs)
}

/// How long the engine has been running, as `sp_mgGetActivityInfo` writes
/// it.
fn up_time() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    let seconds = START.get_or_init(Instant::now).elapsed().as_secs();
    format!(
        "{} Days {} Hours {} Minutes {} Seconds",
        seconds / 86400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn int(n: impl Into<i128>) -> Value {
    Value::Integer(n.into())
}

fn ci(name: &str, length: u16) -> Column {
    column(name, FieldType::CiCharacter, length)
}

fn integer(name: &str) -> Column {
    column(name, FieldType::Integer, 4)
}

fn results(columns: Vec<Column>, rows: Vec<Vec<Value>>) -> Result<Option<Results>, Error> {
    Ok(Some(Results::new(columns, rows)))
}

/// The columns of a result that describes a user.
fn user_columns() -> Vec<Column> {
    vec![
        ci("UserName", 50),
        integer("ConnNumber"),
        ci("DictionaryUser", 50),
        ci("Address", 30),
        ci("OSUserLoginName", 50),
        ci("TSAddress", 30),
        ci("ApplicationID", 70),
    ]
}

/// A row that describes `connection`'s user: the OS user's name, and the
/// application ID; the rest is empty on a local server.
fn user_row(session: &Session, connection: Handle) -> Vec<Value> {
    let id = connection::application_id(session, connection).unwrap_or_default();
    let dictionary_user = connection::get(session, connection)
        .ok()
        .and_then(|owner| owner.user().map(str::to_owned))
        .unwrap_or_default();
    vec![
        text(&crate::connection::os_user()),
        int(0),
        text(&dictionary_user),
        text(""),
        text(""),
        text(""),
        text(&id),
    ]
}

/// What a table's locking mode is called, and its number.
fn locking(table_type: TableType) -> (&'static str, i32) {
    match table_type {
        TableType::Cdx | TableType::Vfp => ("CDX", 2),
        TableType::Ntx => ("NTX", 3),
        TableType::Adt | TableType::Database => ("ADT", 4),
    }
}

fn table_columns() -> Vec<Column> {
    vec![
        ci("TableName", 255),
        ci("LockType", 5),
        integer("LockTypeValue"),
    ]
}

fn table_rows(tables: Vec<handle::OpenTable>) -> Vec<Vec<Value>> {
    tables
        .into_iter()
        .map(|open| {
            let (kind, value) = locking(open.table_type);
            vec![text(&open.path), text(kind), int(value)]
        })
        .collect()
}

/// The ranges of record numbers a call asks about, normalised for comparing
/// names: regardless of case, and of `\` or `/`.
fn normal(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// The record numbers locked in the tables open on the file `file`, by any
/// handle of the session.
fn locks_of(session: &Session, file: &str) -> Vec<u32> {
    let wanted = normal(file);
    let mut records = Vec::new();
    for handle in handle::tables(session, 0) {
        let Ok(opened) = table::get(session, handle) else {
            continue;
        };
        let path = normal(&opened.path.to_string_lossy());
        if path == wanted || path.ends_with(&format!("/{wanted}")) {
            records.extend(opened.locks());
        }
    }
    records
}

/// Runs the procedure `execute` calls with `args` on `connection`.
pub(crate) fn run(
    session: &Session,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    let spec = find(&execute.name).ok_or(Error::NotImplemented)?;
    // Those only the root dictionary's administrator may run are refused
    // whatever they are given, but too many arguments.
    if matches!(
        spec.name,
        "SP_MGGETCRASHDUMPINFO" | "SP_MGGETERRORLOG" | "SP_MGSETCONFIGVALUE"
    ) {
        if spec.name == "SP_MGGETERRORLOG" && args.len() > 2 {
            return Err(Error::Sql(SqlError::procedure_parameters(
                &execute.name,
                execute.pos,
            )));
        }
        // A dictionary connection's refusal has no procedure named.
        let dictionary = connection::get(session, connection)
            .ok()
            .is_some_and(|owner| owner.with_database_opt(|_| ()).is_some());
        return Err(match dictionary {
            true => ace_error::sql_bare(5054),
            false => ace_error::sql(5054, &execute.name),
        });
    }
    let args = check_arguments(spec, execute, args)?;
    let user = crate::connection::os_user();
    // A user the call names is the OS user or no one, whatever the case.
    let is_ours = |name: Option<String>| name.is_some_and(|n| n.eq_ignore_ascii_case(&user));
    match spec.name {
        "SP_SETSTATEMENTLIMIT" => {
            let limit = args.integer(0).unwrap_or(0);
            connection::set_statement_limit(session, connection, limit as i32)?;
            Ok(None)
        }
        // A connection's requests queue as it asks; nothing here queues.
        "SP_SETREQUESTPRIORITY" => Ok(None),
        // The original's local server cancels nothing. A server's session
        // cancels the waits for events, the only statements here that run
        // long enough to be cancelled.
        "SP_CANCELQUERY" => match u32::try_from(args.integer(0).unwrap_or(0)) {
            _ if session.shares.is_none() => Err(Error::Sql(SqlError::ace_bare(
                5042,
                "The requested feature has not yet been implemented.",
            ))),
            Ok(number) if crate::events::cancel(number) => Ok(None),
            _ => Err(ace_error::sql_bare(5187)),
        },
        "SP_MGRESETCOMMSTATS" => Ok(None),
        "SP_MGKILLUSER" => match args.text(0).as_deref() {
            None | Some("") => Err(ace_error::sql(5100, "Parameter 2")),
            // As the original's local server, which has no one to disconnect.
            Some(_) => Ok(None),
        },
        "SP_MGKILLUSER90" => match args.text(0).as_deref() {
            None | Some("") => Err(ace_error::sql(5100, "Parameter 2")),
            // Only 0 to 2 and NULL say what the user is found by.
            Some(_)
                if args
                    .integer(1)
                    .is_some_and(|property| !(0..=2).contains(&property)) =>
            {
                Err(ace_error::sql(
                    5126,
                    "The specified property cannot be used to kill a user.",
                ))
            }
            Some(_) => Ok(None),
        },
        "SP_MGGETSERVERTYPE" => {
            let (name, value) = match session.shares.is_some() {
                false => ("Local Server", 3),
                true if cfg!(windows) => ("Windows Server", 2),
                true => ("Linux", 6),
            };
            results(
                vec![
                    ci("ServerType", 20),
                    integer("ServerTypeValue"),
                    integer("OSMajor"),
                    integer("OSMinor"),
                ],
                vec![vec![text(name), int(value), int(0), int(0)]],
            )
        }
        "SP_MGGETACTIVITYINFO" => results(
            vec![
                integer("Operations"),
                integer("LoggedErrors"),
                column("UpTime", FieldType::Character, 50),
                integer("EQThreshold"),
                integer("EQActiveThreads"),
                integer("EQOperations"),
            ],
            vec![vec![
                int(0),
                int(0),
                text(&up_time()),
                int(0),
                int(0),
                int(0),
            ]],
        ),
        "SP_MGGETCOMMSTATS" => {
            let mut columns = vec![Column {
                name: "PercentCheckSums".to_owned(),
                field_type: FieldType::Double,
                length: 8,
                decimals: 2,
            }];
            let mut row = vec![Value::Double(0.0)];
            for name in [
                "TotalPackets",
                "RcvPktOutOfSeq",
                "NotLoggedIn",
                "RcvReqOutOfSeq",
                "CheckSumFailures",
                "DisconnectedUsers",
                "PartialConnects",
                "InvalidPackets",
                "RcvFromErrors",
                "SendToErrors",
            ] {
                columns.push(integer(name));
                row.push(int(0));
            }
            results(columns, vec![row])
        }
        "SP_MGGETCONFIGINFO" => {
            let limit = connection::statement_limit(session, connection)?;
            // A server's sessions answer as the original's remote server does
            // (the evaluation server's defaults, probed), the client library's
            // as its local server.
            let server = session.shares.is_some();
            let pick = |local: &str, remote: &str| (if server { remote } else { local }).to_owned();
            let items: [(&str, String); 25] = [
                ("Stat Dump Interval", "0".into()),
                ("Error Log Max", "1000".into()),
                ("Error Log Path", "C:\\".into()),
                ("Burst Packets", pick("0", "16")),
                ("Sort Buffer", pick("32768", "8192")),
                ("Send IP Port", "0".into()),
                ("Receive IP Port", pick("0", "6262")),
                ("Ghost Timeout", pick("0", "120000")),
                ("Flush Frequency", pick("20000", "0")),
                ("Keep Alive Timeout", pick("0", "120")),
                ("Internet Port", pick("0", "6270")),
                ("Max Conn Failures", pick("0", "20")),
                ("Internet Keep Alive", pick("0", "120")),
                ("Compression Level", pick("0", "12")),
                ("Use IP", "0".into()),
                ("Flush Every Update", "0".into()),
                ("NetWare Login Names", "0".into()),
                ("Use Semaphore Files", "0".into()),
                ("Semaphore File Path", String::new()),
                ("Use Dynamic AOFs", "1".into()),
                ("Use Internet", pick("0", "1")),
                ("Transaction Log Path", pick("", "C:\\")),
                ("Receive SSL Port", pick("0", "6263")),
                ("SQL Statement Limit", "50".into()),
                ("User SQL Statement Limit", limit.to_string()),
            ];
            results(
                vec![ci("Item", 30), ci("Value", 256)],
                items
                    .into_iter()
                    .map(|(item, value)| vec![text(item), text(&value)])
                    .collect(),
            )
        }
        "SP_MGGETCONFIGMEMORY" | "SP_MGGETCONFIGMEMORY90" => {
            // What the configuration takes: the numbers the original's local
            // server reports for its defaults.
            let items = [
                ("Connections", 20, 130_560),
                ("Work Areas", 100, 104_000),
                ("Tables", 50, 43_600),
                ("Indexes", 75, 30_300),
                ("Locks", 500, 18_000),
                ("User Buffer Size", 534, 11_214),
                ("TPS Header Elems", 0, 0),
                ("TPS Visibility Elems", 0, 0),
                ("TPS Memo Elems", 0, 0),
                ("Receive ECBs", 0, 0),
                ("Send ECBs", 0, 0),
                ("Worker Threads", 0, 0),
            ];
            let total: i32 = items.iter().map(|item| item.2).sum();
            let mut rows: Vec<Vec<Value>> = items
                .into_iter()
                .map(|(item, value, memory)| vec![text(item), int(value), int(memory)])
                .collect();
            rows.push(vec![text("Total Memory"), int(1), int(total)]);
            // The 90 form's memory is a Numeric, which holds more than 4 GB;
            // its total is 0, though the items add up (see ADS_BUGS.md).
            if spec.name == "SP_MGGETCONFIGMEMORY90" {
                for row in &mut rows {
                    if let Value::Integer(memory) = row[2] {
                        row[2] = Value::Numeric(format!("{memory:>30}"));
                    }
                }
                if let Some(total) = rows.last_mut() {
                    total[2] = Value::Numeric(format!("{:>30}", 0));
                }
                return results(
                    vec![
                        ci("Item", 30),
                        integer("Value"),
                        column("Memory", FieldType::Numeric, 30),
                    ],
                    rows,
                );
            }
            results(
                vec![ci("Item", 30), integer("Value"), integer("Memory")],
                rows,
            )
        }
        // What the original's cache holds depends on what its process has
        // done; this one never holds anything.
        "SP_MGGETCACHEINFO" => {
            let long = |name: &str| column(name, FieldType::LongInt, 8);
            results(
                vec![
                    integer("TotalBlocks"),
                    long("TotalMemory"),
                    integer("TotalAllocations"),
                    integer("TotalDeallocations"),
                    integer("TotalCacheHits"),
                    long("CacheLimit"),
                    long("PeakAlloc"),
                    long("WorkingSet"),
                    long("TotalHeap"),
                    integer("CacheClosedTempTables"),
                    integer("CacheClosedTempIndexes"),
                ],
                vec![
                    [0, 0, 0, 0, 0, 268_435_456, 0, 0, 0, 0, 0]
                        .map(int)
                        .to_vec(),
                ],
            )
        }
        // The original lists the files its cache holds for a table it has
        // open, given by the path it opened it with: the table, its memo file
        // and its index file. A table open here is the same.
        "SP_MGGETTABLECACHEINFO" => {
            let wanted = args.text(0).map(|file| normal(&file)).unwrap_or_default();
            let mut rows = Vec::new();
            let open = handle::open_tables(session, false)
                .into_iter()
                .find(|open| normal(&open.path) == wanted);
            if let Some(open) = open {
                let table = std::path::Path::new(&open.path);
                let others = ["adm", "fpt", "dbt", "adi", "cdx", "ntx"]
                    .into_iter()
                    .map(|extension| table.with_extension(extension))
                    .filter(|file| file.is_file());
                let files = std::iter::once(open.path.clone())
                    .chain(others.map(|file| file.to_string_lossy().into_owned()));
                for file in files {
                    rows.push(vec![
                        text(&file),
                        Value::Logical(false),
                        int(0),
                        int(0),
                        int(0),
                        Value::Numeric(format!("{:>20}", 0)),
                        Value::Logical(false),
                        Value::Logical(true),
                    ]);
                }
            }
            results(
                vec![
                    column("FileName", FieldType::Character, 255),
                    column("CacheEnabled", FieldType::Logical, 1),
                    integer("CachedBlocks"),
                    integer("HeaderSize"),
                    integer("PageSize"),
                    column("FileLength", FieldType::Numeric, 20),
                    column("CacheUpdates", FieldType::Logical, 1),
                    column("HasPhysicalFile", FieldType::Logical, 1),
                ],
                rows,
            )
        }
        "SP_MGGETUSAGEINFO" => {
            let activity = handle::activity(session);
            // (item, in use, most used, configured, rejected)
            let items = [
                ("Users", 0, 0, 5),
                // As the original's local server, which never has a most used.
                ("Connections", activity.connections, 0, 20),
                ("Work Areas", activity.work_areas, activity.work_areas, 100),
                ("Tables", activity.tables, activity.tables, 50),
                ("Indexes", activity.indexes, activity.indexes, 75),
                ("Locks", 0, 0, 500),
                ("TPS Header Elems", 0, 0, 0),
                ("TPS Visibility Elems", 0, 0, 0),
                ("TPS Memo Elems", 0, 0, 0),
                ("Worker Threads", u32::MAX, 0, 0),
                ("Stateful Users", 0, 0, 5),
                ("Stateless Users", 0, 0, 0),
            ];
            results(
                vec![
                    ci("Item", 20),
                    integer("InUse"),
                    integer("MaxUsed"),
                    integer("Configured"),
                    integer("Rejected"),
                ],
                items
                    .into_iter()
                    .map(|(item, used, most, configured)| {
                        vec![
                            text(item),
                            int(used as i32),
                            int(most as i32),
                            int(configured),
                            int(0),
                        ]
                    })
                    .collect(),
            )
        }
        "SP_MGGETCONNECTEDUSERS" => {
            let mut columns = user_columns();
            columns.push(integer("AverageCost"));
            let rows = handle::connections(session)
                .into_iter()
                .map(|(handle, _)| {
                    let mut row = user_row(session, handle);
                    row.push(int(0));
                    row
                })
                .collect();
            results(columns, rows)
        }
        "SP_MGGETALLTABLES" => results(
            table_columns(),
            table_rows(handle::open_tables(session, false)),
        ),
        "SP_MGGETALLINDEXES" => results(
            vec![ci("IndexName", 255)],
            handle::open_indexes(session, None, false)?
                .iter()
                .map(|path| vec![text(path)])
                .collect(),
        ),
        "SP_MGGETWORKERTHREADACTIVITY" => results(
            vec![
                integer("ThreadNumber"),
                integer("OpCode"),
                ci("UserName", 50),
                integer("ConnNumber"),
                ci("OSUserLoginName", 50),
            ],
            Vec::new(),
        ),
        "SP_MGGETALLLOCKS" => {
            let records = args.text(0).map(|file| locks_of(session, &file));
            results(
                vec![integer("LockedRecNo")],
                records
                    .unwrap_or_default()
                    .into_iter()
                    .map(|record| vec![int(record)])
                    .collect(),
            )
        }
        "SP_MGGETUSERLOCKS" => {
            let records = match is_ours(args.text(1)) {
                true => args.text(0).map(|file| locks_of(session, &file)),
                false => None,
            };
            results(
                vec![integer("LockedRecNo")],
                records
                    .unwrap_or_default()
                    .into_iter()
                    .map(|record| vec![int(record)])
                    .collect(),
            )
        }
        "SP_MGGETLOCKOWNER" => {
            let Some(file) = args.text(0).filter(|file| !file.is_empty()) else {
                return Err(ace_error::sql(5020, "Parameter 2"));
            };
            // The file must be open on the server.
            handle::users(session, Some(&file)).map_err(|error| match error {
                Error::FileNotOpen => ace_error::sql(7051, ""),
                error => ace_error::sql_error(error),
            })?;
            let record = args.integer(1).unwrap_or(0) as u32;
            let locked = locks_of(session, &file).contains(&record);
            let who = handle::connections(session)
                .first()
                .map(|&(handle, _)| user_row(session, handle));
            let mut row = match (locked, who) {
                (true, Some(row)) => row,
                _ => vec![
                    text(""),
                    int(0),
                    text(""),
                    text(""),
                    text(""),
                    text(""),
                    text(""),
                ],
            };
            // The lock's type goes after the address.
            row.insert(4, text(if locked { "Record Lock" } else { "No Lock" }));
            results(
                vec![
                    ci("UserName", 50),
                    integer("ConnNumber"),
                    ci("DictionaryUser", 50),
                    ci("Address", 30),
                    ci("LockType", 15),
                    ci("OSUserLoginName", 50),
                    ci("TSAddress", 30),
                    ci("ApplicationID", 70),
                ],
                vec![row],
            )
        }
        "SP_MGGETTABLEINDEXES" | "SP_MGGETINDEXUSERS" | "SP_MGGETTABLEUSERS" => {
            let file = args.text(0).filter(|file| !file.is_empty());
            // A file no one has open lists no one, and is no error.
            let open = |file: &str| handle::users(session, Some(file)).is_ok();
            match spec.name {
                "SP_MGGETTABLEINDEXES" => {
                    let paths = file
                        .and_then(|file| handle::open_indexes(session, Some(&file), true).ok())
                        .unwrap_or_default();
                    results(
                        vec![ci("IndexName", 255)],
                        paths.iter().map(|path| vec![text(path)]).collect(),
                    )
                }
                "SP_MGGETTABLEUSERS" => {
                    let users = file
                        .filter(|file| open(file))
                        .map_or(0, |file| handle::users(session, Some(&file)).unwrap_or(0));
                    let rows = handle::connections(session)
                        .into_iter()
                        .take(users as usize)
                        .map(|(handle, _)| user_row(session, handle))
                        .collect();
                    results(user_columns(), rows)
                }
                // Who has the index file open: the files' tables' users.
                _ => {
                    let wanted = file.map(|file| normal(&file));
                    let opened = handle::open_indexes(session, None, false).unwrap_or_default();
                    let open = wanted
                        .is_some_and(|wanted| opened.iter().any(|path| normal(path) == wanted));
                    let rows = match open {
                        true => handle::connections(session)
                            .into_iter()
                            .map(|(handle, _)| user_row(session, handle))
                            .collect(),
                        false => Vec::new(),
                    };
                    results(user_columns(), rows)
                }
            }
        }
        "SP_MGGETUSERTABLES" => {
            let tables = match is_ours(args.text(0)) {
                true => handle::open_tables(session, true),
                false => Vec::new(),
            };
            results(table_columns(), table_rows(tables))
        }
        _ => {
            // SP_MGGETUSERINDEXES
            let paths = match is_ours(args.text(0)) {
                true => handle::open_indexes(session, None, true)?,
                false => Vec::new(),
            };
            results(
                vec![ci("IndexName", 255)],
                paths.iter().map(|path| vec![text(path)]).collect(),
            )
        }
    }
}
