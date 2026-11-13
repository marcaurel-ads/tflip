//! The system procedures of event notifications (see [`crate::events`]):
//! `sp_CreateEvent`, `sp_DropEvent`, `sp_DropAllEvents`, `sp_SignalEvent`,
//! `sp_WaitForEvent` and `sp_WaitForAnyEvent`.

use tflip_sql::ast::Execute;
use tflip_sql::{SqlError, V};
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use super::Results;
use super::args::{Kind, Spec, check_arguments, column};
use crate::events::{self, Context};
use crate::handle::Handle;
use crate::{Error, Session, connection};

const NAME: (&str, Kind) = ("EventName", Kind::Char(200));
const OPTIONS: (&str, Kind) = ("Options", Kind::Integer);
const TIMEOUT: (&str, Kind) = ("Timeout", Kind::Integer);
const POLL: (&str, Kind) = ("PollInterval", Kind::Integer);

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

const SPECS: [Spec; 6] = [
    spec("SP_CREATEEVENT", &[NAME, OPTIONS], 2, false),
    spec("SP_DROPEVENT", &[NAME, OPTIONS], 2, false),
    spec("SP_DROPALLEVENTS", &[OPTIONS], 1, false),
    spec(
        "SP_SIGNALEVENT",
        &[
            NAME,
            ("WaitForCommit", Kind::Flag),
            OPTIONS,
            ("EventData", Kind::Memo),
        ],
        3,
        false,
    ),
    spec("SP_WAITFOREVENT", &[NAME, TIMEOUT, POLL, OPTIONS], 4, true),
    spec("SP_WAITFORANYEVENT", &[TIMEOUT, POLL, OPTIONS], 3, true),
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

/// The parameters of the procedure `name`, as fields.
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    find(name).map_or_else(Vec::new, super::args::inputs)
}

/// Runs the procedure `execute` calls with `args`, called from `context`, as
/// the statement `statement` (a handle that is not a statement's if it is
/// none).
pub(crate) fn run(
    session: &Session,
    statement: Handle,
    handle: Handle,
    execute: &Execute,
    args: &[V],
    context: Context,
) -> Result<Option<Results>, Error> {
    let spec = find(&execute.name).ok_or(Error::NotImplemented)?;
    let args = check_arguments(spec, execute, args)?;
    let owner = connection::get(session, handle)?;
    let cut = args.cut;
    // A NULL name is the empty one, a NULL number 0.
    let name = args.text(0).unwrap_or_default();
    let number = |i: usize| args.integer(i).unwrap_or(0);
    let done = match spec.name {
        "SP_CREATEEVENT" => events::create(&owner, &name, number(1)).map(|()| None),
        "SP_DROPEVENT" => events::drop_event(&owner, &name).map(|()| None),
        "SP_DROPALLEVENTS" => {
            events::drop_all(&owner);
            Ok(None)
        }
        "SP_SIGNALEVENT" => {
            let wait_for_commit = match args.values.get(1) {
                Some(V::Bool(b)) => *b,
                _ => number(1) != 0,
            };
            let data = args.text(3);
            events::signal(&owner, context, &name, wait_for_commit, data).map(|()| None)
        }
        wait => {
            let any = wait == "SP_WAITFORANYEVENT";
            let timeout = number(if any { 0 } else { 1 });
            let statement = crate::statement::get(session, statement).map_or(0, |s| s.number);
            let found = events::wait(
                &owner,
                context,
                statement,
                (!any).then_some(&*name),
                timeout,
            )?;
            let columns = vec![
                column("EventName", FieldType::CiCharacter, 200),
                column("EventCount", FieldType::Integer, 4),
                column("StringData", FieldType::Memo, 9),
            ];
            let text = |s: String| Value::Text(s);
            let mut rows: Vec<Vec<Value>> = found
                .notices
                .into_iter()
                .map(|n| {
                    vec![
                        text(n.name),
                        Value::Integer(i128::from(n.count)),
                        n.data.map_or(Value::Null, text),
                    ]
                })
                .collect();
            // A wait that timed out answers a row of no signals, named as
            // the call named the event, or with no name.
            if rows.is_empty() {
                rows.push(vec![text(name.clone()), Value::Integer(0), Value::Null]);
                if any {
                    rows[0][0] = text(String::new());
                }
            }
            Ok(Some(Results::new(columns, rows)))
        }
    };
    // A name longer than the parameter is cut, and the call complains of it
    // once it has run.
    match cut {
        true => done.and(Err(Error::Sql(SqlError::input_too_large()))),
        false => done,
    }
}
