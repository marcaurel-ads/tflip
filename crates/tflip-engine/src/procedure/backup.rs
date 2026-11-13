//! The online backup and restore procedures (`sp_BackupDatabase`,
//! `sp_RestoreDatabase`, `sp_BackupFreeTables`, `sp_RestoreFreeTables`): what
//! they refuse, in the order the original refuses it
//! (`fixtures/backup/README.md`); the work is in [`crate::backup`].

use tflip_sql::V;
use tflip_sql::ast::Execute;

use super::Results;
use super::args::{Arguments, Kind, Spec, check_arguments};
use crate::backup::database::{self, Restore};
use crate::backup::free;
use crate::backup::options::{Operation, Options};
use crate::handle::Handle;
use crate::{Error, Session, ace_error, connection};

const PATH: Kind = Kind::Memo;

const SPECS: [Spec; 4] = [
    Spec {
        name: "SP_BACKUPDATABASE",
        params: &[("DestPath", PATH), ("Options", Kind::Options)],
        required: 2,
        results: true,
    },
    Spec {
        name: "SP_RESTOREDATABASE",
        params: &[
            ("SourcePath", PATH),
            ("SourcePassword", Kind::Memo),
            ("DestPath", PATH),
            ("Options", Kind::Options),
        ],
        required: 4,
        results: true,
    },
    Spec {
        name: "SP_BACKUPFREETABLES",
        params: &[
            ("SourcePath", PATH),
            ("SourceMask", Kind::Memo),
            ("DestPath", PATH),
            ("Options", Kind::Options),
            ("FreetablePasswords", Kind::Memo),
        ],
        required: 5,
        results: true,
    },
    Spec {
        name: "SP_RESTOREFREETABLES",
        params: &[
            ("SourcePath", PATH),
            ("DestPath", PATH),
            ("Options", Kind::Options),
            ("FreetablePasswords", Kind::Memo),
        ],
        required: 4,
        results: true,
    },
];

fn find(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// Whether `name` is one of these procedures.
pub(crate) fn handles(name: &str) -> bool {
    find(name).is_some()
}

/// The parameters of the procedure `name`, as fields.
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    find(name).map_or_else(Vec::new, super::args::inputs)
}

/// The refusal of a call that names no path, worded with the number of the
/// parameter of the ACE call that gave it.
fn no_pointer(parameter: u32) -> Error {
    ace_error::sql(5021, &format!("Parameter {parameter}"))
}

/// A path argument, which the call may not leave out or empty.
fn path(args: &Arguments, at: usize, parameter: u32) -> Result<String, Error> {
    args.text(at)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| no_pointer(parameter))
}

/// Whether the user of the connection may call the procedure that needs
/// `DB:Backup`: a free connection never may.
fn check_user(session: &Session, handle: Handle, free_refused: Option<&str>) -> Result<(), Error> {
    let owner = connection::get(session, handle)?;
    match owner.with_database_opt(|database| database.may_back_up()) {
        Some(true) => Ok(()),
        Some(false) => Err(ace_error::sql(
            5054,
            "Must be administrator or member of DB:Backup to use this functionality.",
        )),
        None => match free_refused {
            Some(call) => Err(ace_error::sql(
                5054,
                &format!("Cannot call {call} on a free connection."),
            )),
            None => Ok(()),
        },
    }
}

/// Runs the procedure `execute` calls with `args` on `connection`.
pub(crate) fn run(
    session: &Session,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    let spec = find(&execute.name).ok_or(Error::NotImplemented)?;
    let args = check_arguments(spec, execute, args)?;
    // Only a server backs up and restores: the local one answers so for any
    // arguments (`fixtures/backup/s30_local_local`).
    if session.shares.is_none() {
        return Err(ace_error::sql(
            5042,
            "Backup and restore functionality is not supported on Advantage Local Server \
             connections.",
        ));
    }
    let options =
        |at: usize, operation| Options::parse(&args.text(at).unwrap_or_default(), operation);
    let rows = match spec.name {
        "SP_BACKUPDATABASE" => {
            let dest = path(&args, 0, 5)?;
            check_user(session, connection, Some("BackupDatabase"))?;
            let options = options(1, Operation::Backup)?;
            database::backup(session, connection, &dest, &options)?
        }
        "SP_RESTOREDATABASE" => {
            let source = path(&args, 0, 3)?;
            let dest = path(&args, 2, 5)?;
            check_user(session, connection, None)?;
            let options = options(3, Operation::Restore)?;
            let password = args.text(1);
            database::restore(
                session,
                &Restore {
                    source: &source,
                    dest: &dest,
                    password: password.as_deref(),
                    options: &options,
                },
            )?
        }
        "SP_BACKUPFREETABLES" => {
            if args.text(1).is_none_or(|mask| mask.is_empty()) {
                return Err(ace_error::sql_bare(5021));
            }
            let (source, mask, dest) = (
                args.text(0).unwrap_or_default(),
                args.text(1).unwrap_or_default(),
                args.text(2).unwrap_or_default(),
            );
            let options = options(3, Operation::Backup)?;
            let passwords = args.text(4);
            free::backup(&free::Backup {
                source: &source,
                mask: &mask,
                dest: &dest,
                options: &options,
                passwords: passwords.as_deref(),
                default_type: connection::get(session, connection)?.table_type(),
            })?
        }
        _ => {
            let (source, dest) = (
                args.text(0).unwrap_or_default(),
                args.text(1).unwrap_or_default(),
            );
            let options = options(2, Operation::Restore)?;
            let passwords = args.text(3);
            free::restore(&free::Restore {
                source: &source,
                dest: &dest,
                options: &options,
                passwords: passwords.as_deref(),
            })?
        }
    };
    Ok(Some(rows.into_results()))
}
