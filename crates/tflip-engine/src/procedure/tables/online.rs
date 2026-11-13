//! `sp_PackTableOnline`, `sp_ReindexOnline` and `sp_PackAllTablesOnline`, which
//! a server runs while other connections go on using the table
//! (`fixtures/events/events_online_procs.json`, `fixtures/sql/online_procs.json`).
//!
//! They work like `CREATE ONLINE INDEX` (see [`crate::maintenance`]): the
//! other connections read and write the table until the final stage, which
//! waits for the transactions on the table to end, signals
//! `__OPFinalStage_<table>` (a reindex signals nothing) and then takes the
//! table to do the work. The `Timeout` option (seconds, 0 or none for as long
//! as it takes) bounds all that waiting: 7176 for a pack, 7189 for a reindex.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tflip_sql::SqlError;

use super::{Recorded, Target};
use crate::ace_error::{self, error_text};
use crate::maintenance::Kind;
use crate::procedure::args::Arguments;
use crate::{Error, connection, ddl, index, shared};

const POLL: Duration = Duration::from_millis(5);

/// What a call is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Work {
    Pack,
    Reindex,
}

impl Work {
    /// The error of the timeout expiring.
    fn timed_out(self) -> Error {
        let code = match self {
            Work::Pack => 7176,
            Work::Reindex => 7189,
        };
        bare(code)
    }
}

/// The ACE error `code` as the SQL engine words it here, without the code.
fn bare(code: u32) -> Error {
    Error::Sql(SqlError::ace_bare(
        code,
        error_text(code).unwrap_or_default(),
    ))
}

/// The options of a call: `Name=Value` entries separated by `;`.
#[derive(Default)]
struct Options {
    /// The seconds the call may wait; none, or 0, is as long as it takes.
    timeout: Option<Duration>,
    memo_block_size: Option<i64>,
    /// Index files besides the table's own, which a reindex rebuilds too.
    indexes: Vec<String>,
}

fn invalid(detail: &str) -> Error {
    ace_error::sql(5019, detail)
}

/// The number an option's value starts with, as the original reads it
/// (`5.5` is 5); none if it does not start with one.
fn leading_number(value: &str) -> Option<i64> {
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

impl Options {
    fn parse(text: Option<&str>, work: Work, all: bool) -> Result<Options, Error> {
        let mut options = Options::default();
        for entry in text.unwrap_or_default().split(';') {
            if entry.is_empty() {
                continue;
            }
            let (name, value) = match entry.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (entry, None),
            };
            // Only a pack takes a memo block size, and a reindex of one
            // table the index files.
            let known = match name.to_ascii_lowercase().as_str() {
                "timeout" => Some("Timeout"),
                "indexes" => Some("Indexes"),
                "memoblocksize" if work == Work::Pack || all => Some("MemoBlockSize"),
                _ => None,
            };
            let Some(known) = known else {
                return Err(invalid(entry));
            };
            let Some(value) = value else {
                return Err(invalid(&format!("missing = after the {known} option")));
            };
            let number = || {
                leading_number(value).ok_or_else(|| {
                    invalid(&format!(
                        "error converting {known} option value to an integer"
                    ))
                })
            };
            match known {
                "Timeout" => {
                    options.timeout = Some(Duration::from_secs(number()? as u64))
                        .filter(|timeout| !timeout.is_zero());
                }
                "MemoBlockSize" => options.memo_block_size = Some(number()?),
                _ => {
                    options.indexes = value
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
            }
        }
        Ok(options)
    }
}

/// How long a wait goes on.
#[derive(Clone, Copy)]
pub(super) enum Wait {
    Forever,
    Until(Instant),
}

impl Wait {
    fn of(options: &Options) -> Wait {
        match options.timeout {
            Some(timeout) => Wait::Until(Instant::now() + timeout),
            None => Wait::Forever,
        }
    }

    /// Whether the wait is over.
    pub(super) fn over(self) -> bool {
        match self {
            Wait::Forever => false,
            Wait::Until(end) => Instant::now() >= end,
        }
    }
}

/// `name` without the extension of an ADT table's file, as the events and
/// the dictionary name the table.
fn bare_name(name: &str) -> &str {
    let name = name.trim_end();
    match name.len().checked_sub(4) {
        Some(at) if name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".adt") => {
            &name[..at]
        }
        _ => name,
    }
}

pub(super) fn run(target: &Target, procedure: &str, args: &Arguments) -> Result<(), Error> {
    let (work, all) = match procedure {
        "SP_PACKTABLEONLINE" => (Work::Pack, false),
        "SP_REINDEXONLINE" => (Work::Reindex, false),
        _ => (Work::Pack, true),
    };
    let options = Options::parse(args.text(if all { 0 } else { 1 }).as_deref(), work, all)?;
    if all {
        let tables = target.all_table_names()?;
        for name in tables {
            one(target, work, &name, &options)?;
        }
        return Ok(());
    }
    let name = args.text(0).unwrap_or_default();
    if name.trim().is_empty() {
        return Err(match (work, target.dictionary()) {
            (Work::Pack, true) => {
                ace_error::sql(5021, "Parameter 2. Table name cannot be an empty string.")
            }
            // A free table's pack looks for the file `.adt`.
            (Work::Pack, false) => {
                let folder = connection::get(target.session, target.connection)?.folder();
                ace_error::sql_error(
                    Error::FileNotFound.detailed(folder.join(".adt").display().to_string()),
                )
            }
            (Work::Reindex, _) => ace_error::sql(5020, "Must specify a table name"),
        });
    }
    one(target, work, &name, &options)
}

/// The online pack or reindex of the table `name`.
fn one(target: &Target, work: Work, name: &str, options: &Options) -> Result<(), Error> {
    let name = bare_name(name);
    let wait = Wait::of(options);
    // The table must be there before the options are held to it.
    let handle = target.open(name, 0, "")?;
    let path = crate::table::get(target.session, handle).map(|t| t.path().to_path_buf());
    let _ = crate::table::close(target.session, handle);
    let path = path
        .ok()
        .or_else(|| ddl::table_path(target.session, target.connection, target.settings, name));
    check_indexes(path.as_deref(), &options.indexes)?;
    let block_size = match (work, options.memo_block_size) {
        (Work::Pack, Some(size)) => target.memo_block_size_online(name, size)?,
        _ => None,
    };
    // A table with no index file has nothing to wait for the transactions
    // on it to rebuild (`fixtures/events/events_online_procs.json`).
    let adt = |path: &PathBuf| {
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("adt"))
    };
    if work == Work::Reindex
        && path
            .as_ref()
            .is_some_and(|path| adt(path) && !path.with_extension("adi").exists())
    {
        return Ok(());
    }
    let event = match work {
        Work::Pack => "__OPFinalStage_",
        Work::Reindex => "",
    };
    // Another online operation on the table is in the way until it is done.
    let gate = loop {
        match ddl::maintain(
            target.session,
            target.connection,
            target.settings,
            name,
            Kind::Online,
            event,
        ) {
            Err(error) if *error.plain() == Error::FileInUse => {
                if wait.over() {
                    return Err(work.timed_out());
                }
                std::thread::sleep(POLL);
            }
            done => break done?,
        }
    };
    let gate = gate.map(|gate| match work {
        Work::Pack => gate,
        Work::Reindex => gate.silent(),
    });
    // The final stage comes when no transaction holds the table.
    if let Some(path) = &path {
        while shared::transactions_held(path) {
            if wait.over() {
                return Err(work.timed_out());
            }
            std::thread::sleep(POLL);
        }
    }
    if let Some(gate) = &gate {
        gate.final_stage();
    }
    match work {
        Work::Pack => {
            target.exclusively_waiting(name, wait, work.timed_out(), |handle| {
                index::pack_blocks(target.session, handle, false, block_size)
            })?;
            if let Some(size) = block_size {
                target.record_in_dictionary(name, Recorded::MemoBlockSize(size))?;
            }
        }
        Work::Reindex => {
            target.exclusively_waiting(name, wait, work.timed_out(), |handle| {
                index::reindex_pages(target.session, handle, None)
            })?;
        }
    }
    Ok(())
}

/// The index files an option names: 5065 for the table's own, which is open
/// already, and 7041 for one that is not there.
fn check_indexes(table: Option<&Path>, indexes: &[String]) -> Result<(), Error> {
    let Some(table) = table else {
        return Ok(());
    };
    let own = table
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase());
    for name in indexes {
        let given = Path::new(name);
        let stem = given
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase());
        let extension = given
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase());
        if stem == own && extension.as_deref().is_none_or(|e| e == "adi") {
            return Err(bare(5065));
        }
        let mut file: PathBuf = table.with_file_name(name);
        if given.extension().is_none() {
            file.set_extension("adi");
        }
        if !file.exists() {
            return Err(bare(7041));
        }
    }
    Ok(())
}
