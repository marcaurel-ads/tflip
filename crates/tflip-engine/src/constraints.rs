//! A data dictionary's own rules on a table's values: field defaults, field
//! constraints (NULL not allowed, a minimum, a maximum, the field's validation
//! message) and the table's record validation expression and message, applied
//! as the original engine applies them (probed, `tools/ace/dd/constraints.c` →
//! `fixtures/dictionary/constraints.json`):
//!
//! - **Defaults** fill a record as it is appended (`AdsAppendRecord`, an SQL
//!   `INSERT`), so they read back before it is written, in field order. A
//!   default is an expression on the record being appended (`NOW()`,
//!   `TODAY()`, `'a' + 'b'`, `2 + 3`, another field), whose value is taken if
//!   the field takes its type (see [`dictionary_value`]); one that is no
//!   expression of the engine's, or of a type the field does not take, is text
//!   set as `AdsSetField` sets it (`USER()`, `01/02/2020`). An explicit NULL is
//!   not replaced by the default. A minimum or maximum is read the same way.
//! - **A field's constraints** are checked as it is set (`AdsSetField`,
//!   `AdsSetNull`, `AdsSetEmpty`, an SQL column, in the order the columns are
//!   listed): a refused value is not set and the field keeps what it held.
//!   NULL passes a minimum and a maximum; an empty memo is NULL.
//! - **The record** is checked as it is written: each field in field order
//!   (NULL, then minimum and maximum: a default may break them, and a row
//!   written before the constraints were set does), then the record
//!   validation expression, which must be true (NULL fails). A record that
//!   fails is not written and stays changed. Triggers fire after these checks.
//!
//! A failure is [`Error::Constraint`], 5147–5150 with the dictionary's message
//! for it if it has one, else the engine's own text naming the column; SQL
//! reports it as the AQE error 7200 with that native error.

use std::cmp::Ordering;
use std::sync::Arc;

use tflip_expr::{Env, Expr, Val};
use tflip_storage::dd::{self, Kind};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::Value;

use crate::handle::Handle;
use crate::setters::{self, Dates, SetValue};
use crate::{Error, Session, connection, filter};

/// A NULL in a field that cannot be NULL.
pub const NOT_NULL: u32 = 5147;
/// A value below the field's minimum.
pub const BELOW_MINIMUM: u32 = 5148;
/// A value above the field's maximum.
pub const ABOVE_MAXIMUM: u32 = 5149;
/// A record its table's validation expression does not hold for.
pub const RECORD_INVALID: u32 = 5150;
/// A trigger that failed.
pub const TRIGGER_FAILED: u32 = 5177;

/// A constraint of a table that a record breaks, in the order a deployment
/// moves the records that break them: the record validation first, then each
/// field's (by its place in the table), its NOT NULL (0), minimum (1) and
/// maximum (2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Broken {
    Record,
    Field(usize, u8),
}

/// One field's rules.
#[derive(Debug, Clone)]
pub(crate) struct FieldRule {
    /// The field's place in the table (0-based).
    pub index: usize,
    default: Option<Bound>,
    required: bool,
    min: Option<Bound>,
    max: Option<Bound>,
    message: Option<String>,
}

/// A default, minimum or maximum: its text, and the text compiled as an
/// expression on the table if it is one.
#[derive(Debug, Clone)]
struct Bound {
    text: String,
    expr: Option<Expr>,
}

/// The rules of a dictionary's table.
#[derive(Debug, Clone)]
pub(crate) struct Checks {
    /// The table's fields.
    fields: Vec<Field>,
    /// The fields with rules, in field order.
    rules: Vec<FieldRule>,
    /// The record validation expression, compiled, and its message.
    record: Option<(Expr, Option<String>)>,
    /// What the expression is evaluated with (its clock and date format are
    /// refreshed at each check).
    env: Env,
}

/// The rules of the dictionary of `connection` on its table `name`, whose
/// fields are `fields`; `None` for a free table or one without rules.
/// `compile` compiles the record validation expression for the table; `env`
/// is what the table evaluates expressions with.
pub(crate) fn load(
    session: &Session,
    connection: Handle,
    name: &str,
    fields: &[Field],
    compile: impl Fn(&str) -> Result<Expr, Error>,
    env: Env,
) -> Option<Arc<Checks>> {
    let short = crate::table::stem(name);
    let owner = connection::get(session, connection).ok()?;
    let (rules, expression, message) = owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let table = dict.named(Kind::Table, &short)?;
            let text = |object: &dd::Object, slot| {
                object.text(slot).filter(|text| !text.trim().is_empty())
            };
            let mut rules = Vec::new();
            for object in dict.children(table.id, Kind::Field) {
                let Some(index) = fields
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(&object.name))
                else {
                    continue;
                };
                let bound = |text: String| Bound {
                    expr: compile(&text).ok(),
                    text,
                };
                let rule = FieldRule {
                    index,
                    // An empty default the dictionary keeps is set as empty
                    // text (an Integer's 0, `fixtures/sql/ddtables.json`).
                    default: object.text(dd::field::DEFAULT_VALUE).map(bound),
                    required: object.has(dd::field::NOT_NULL),
                    min: text(object, dd::field::MIN_VALUE).map(bound),
                    max: text(object, dd::field::MAX_VALUE).map(bound),
                    message: text(object, dd::field::VALIDATION_MSG),
                };
                if rule.default.is_some()
                    || rule.required
                    || rule.min.is_some()
                    || rule.max.is_some()
                {
                    rules.push(rule);
                }
            }
            rules.sort_by_key(|rule| rule.index);
            Some((
                rules,
                text(table, dd::table::VALIDATION_EXPR),
                text(table, dd::table::VALIDATION_MSG),
            ))
        })
        .flatten()?;
    let record = expression
        .and_then(|expression| compile(&expression).ok())
        .map(|e| (e, message));
    if rules.is_empty() && record.is_none() {
        return None;
    }
    Some(Arc::new(Checks {
        fields: fields.to_vec(),
        rules,
        record,
        env,
    }))
}

impl Checks {
    /// The table's fields.
    pub(crate) fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// The date format the table had at open, for a check without a
    /// connection.
    pub(crate) fn date_format(&self) -> String {
        self.env.date_format.clone()
    }

    /// The epoch of two-digit years.
    pub(crate) fn epoch(&self) -> u16 {
        self.env.epoch
    }

    fn rule(&self, index: usize) -> Option<&FieldRule> {
        self.rules.iter().find(|rule| rule.index == index)
    }

    /// Fills the defaults into `record`, the record `number` just appended,
    /// in field order, each read on the record as the defaults before it
    /// left it.
    pub(crate) fn fill_defaults(&self, record: &mut [u8], number: u32, dates: Dates) {
        let env = self.env_now(dates);
        for rule in &self.rules {
            let Some(default) = &rule.default else {
                continue;
            };
            let field = &self.fields[rule.index];
            if field.is_memo() {
                continue;
            }
            let value = self.value_of(default, field, record, number, &env);
            let mut changed = record.to_vec();
            if matches!(
                setters::apply(field, &value, dates, &mut changed),
                Ok(()) | Err(Error::DataTruncated)
            ) {
                record.copy_from_slice(&changed);
            }
        }
    }

    /// The table's environment, its clock and date format brought up to now.
    fn env_now(&self, dates: Dates) -> Env {
        let mut env = self.env.clone();
        let now = crate::clock::now();
        env.now = now.local;
        env.utc = now.utc;
        env.date_format = dates.format.to_owned();
        env.epoch = dates.epoch;
        env
    }

    /// What `bound` sets `field` to, read on `record` (the record `number`).
    fn value_of(
        &self,
        bound: &Bound,
        field: &Field,
        record: &[u8],
        number: u32,
        env: &Env,
    ) -> SetValue {
        let value = bound.expr.as_ref().and_then(|expr| {
            let mut row = filter::Record {
                fields: &self.fields,
                bytes: record.to_vec(),
                memo: None,
                number,
            };
            expr.eval(&mut row, env).ok()
        });
        dictionary_value(field, &bound.text, value)
    }

    /// Checks the field at `index` set to the value in `record` (or, for a
    /// memo, to NULL if `null`), as a client sets it.
    pub(crate) fn check_set(
        &self,
        index: usize,
        record: &[u8],
        null: bool,
        dates: Dates,
    ) -> Result<(), Error> {
        match self.rule(index) {
            Some(rule) => self.check_field(rule, record, null, dates),
            None => Ok(()),
        }
    }

    /// Checks a record about to be written: each field with rules, in field
    /// order (`null` says whether a field is NULL), then the record validation
    /// expression, evaluated by `holds` with the expression and its
    /// environment.
    pub(crate) fn check_record(
        &self,
        record: &[u8],
        null: impl Fn(&Field) -> bool,
        dates: Dates,
        holds: impl FnOnce(&Expr, &Env) -> bool,
    ) -> Result<(), Error> {
        for rule in &self.rules {
            let field = &self.fields[rule.index];
            self.check_field(rule, record, null(field), dates)?;
        }
        if let Some((expr, message)) = &self.record {
            let env = self.env_now(dates);
            if !holds(expr, &env) {
                let text = message.clone().unwrap_or_else(|| {
                    "The record cannot be updated because it does not pass the record level \
                     constraint."
                        .to_owned()
                });
                return Err(Error::Constraint {
                    code: RECORD_INVALID,
                    text,
                });
            }
        }
        Ok(())
    }

    /// The first of the table's constraints, in the order a deployment
    /// takes records out for them, that the stored `record` breaks: the
    /// record validation, then each field's NOT NULL, minimum and maximum, in
    /// field order.
    pub(crate) fn first_broken(
        &self,
        record: &[u8],
        null: impl Fn(&Field) -> bool,
        dates: Dates,
        holds: impl FnOnce(&Expr, &Env) -> bool,
    ) -> Option<Broken> {
        if let Some((expr, _)) = &self.record
            && !holds(expr, &self.env_now(dates))
        {
            return Some(Broken::Record);
        }
        for rule in &self.rules {
            let field = &self.fields[rule.index];
            if let Err(Error::Constraint { code, .. }) =
                self.check_field(rule, record, null(field), dates)
            {
                let part = match code {
                    NOT_NULL => 0,
                    BELOW_MINIMUM => 1,
                    _ => 2,
                };
                return Some(Broken::Field(rule.index, part));
            }
        }
        None
    }

    fn check_field(
        &self,
        rule: &FieldRule,
        record: &[u8],
        null: bool,
        dates: Dates,
    ) -> Result<(), Error> {
        let field = &self.fields[rule.index];
        let fail = |code: u32, what: &str| {
            let text = rule.message.clone().unwrap_or_else(|| {
                format!(
                    "Constraint violation while updating column \"{}\".  The column cannot be \
                     updated to {what}",
                    field.name
                )
            });
            Err(Error::Constraint { code, text })
        };
        if null {
            return match rule.required {
                true => fail(NOT_NULL, "a NULL value due to a 'not NULL' constraint."),
                false => Ok(()),
            };
        }
        if field.is_memo() || (rule.min.is_none() && rule.max.is_none()) {
            return Ok(());
        }
        let Some(value) = key(field, record) else {
            return Ok(());
        };
        let env = self.env_now(dates);
        let bound = |bound: &Option<Bound>| -> Option<Key> {
            let set = self.value_of(bound.as_ref()?, field, record, 0, &env);
            let mut scratch = record.to_vec();
            setters::apply(field, &set, dates, &mut scratch).ok()?;
            key(field, &scratch)
        };
        if let Some(min) = bound(&rule.min)
            && compare(&value, &min) == Ordering::Less
        {
            return fail(
                BELOW_MINIMUM,
                "a value less than the minimum constraint value.",
            );
        }
        if let Some(max) = bound(&rule.max)
            && compare(&value, &max) == Ordering::Greater
        {
            return fail(
                ABOVE_MAXIMUM,
                "a value greater than the maximum constraint value.",
            );
        }
        Ok(())
    }
}

/// Which of a field's values a dictionary write sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Property {
    Default,
    /// The minimum or the maximum.
    Bound,
}

/// Checks `text`, about to be set as the default, minimum or maximum of
/// `field` (`AdsDDSetFieldProperty`, `ADS_NO_VALIDATE` or not), as the
/// original engine checks it (probed, `tools/ace/dd/rules.ops`): memo,
/// binary and the automatic fields take no default, and those and logical and
/// binary-string fields no minimum or maximum (5066); a value must be one the
/// field takes as a default takes it, with the setter's error (and the
/// field's name) if not, and an expression's value of a type the field does
/// not take is 5066 — but for a character field, which takes the text, a
/// Date field, and a timestamp for a Double or Money field. `compile`
/// compiles an expression on the table, `env` what it is evaluated with,
/// `record` an empty record of the table.
pub(crate) fn check_property(
    field: &Field,
    fields: &[Field],
    property: Property,
    text: &str,
    compile: impl Fn(&str) -> Result<Expr, Error>,
    env: &Env,
    record: &[u8],
) -> Result<(), Error> {
    use FieldType::*;
    let name = &field.name;
    let automatic = matches!(field.field_type, AutoInc | RowVersion);
    let unsupported = match property {
        Property::Default if field.is_memo() || automatic => "default field values",
        Property::Bound
            if field.is_memo()
                || automatic
                || matches!(field.field_type, Logical | Raw | VarBinary) =>
        {
            "mininum or maximum values"
        }
        _ => "",
    };
    if !unsupported.is_empty() {
        // "mininum" (sic), as the original says it.
        return Err(Error::InvalidFieldType.detailed(format!(
            "Setting {unsupported} for field data type in column \"{name}\" is not supported."
        )));
    }
    if text.is_empty() {
        return Ok(());
    }
    let expr = compile(text).ok();
    let value = expr.as_ref().and_then(|expr| {
        let mut row = filter::Record {
            fields,
            bytes: record.to_vec(),
            memo: None,
            number: 0,
        };
        expr.eval(&mut row, env).ok()
    });
    let lenient = |value: &Val| {
        is_character(field)
            || field.field_type == Date
            || (matches!(value, Val::TimeStamp(..)) && matches!(field.field_type, Double | Money))
    };
    let set = match value {
        Some(value) => match taken(field, value.clone()) {
            Some(set) => set,
            None if lenient(&value) => SetValue::Field(text.to_owned()),
            None => return Err(Error::InvalidFieldType.detailed(format!("Field: \"{name}\""))),
        },
        None => SetValue::Field(text.to_owned()),
    };
    // A ModTime field takes a timestamp, though no client sets it.
    if field.field_type == ModTime && matches!(set, SetValue::TimeStampRaw(..)) {
        return Ok(());
    }
    let dates = Dates {
        format: &env.date_format,
        epoch: env.epoch,
    };
    let mut scratch = record.to_vec();
    match setters::apply(field, &set, dates, &mut scratch) {
        Ok(()) => Ok(()),
        Err(error) => Err(with_field(error, field)),
    }
}

/// `error`, the value of `field` a setter refused, with what the original
/// engine adds to its text: why, as for a setter (see `AdsSet*`), and the
/// field.
fn with_field(error: Error, field: &Field) -> Error {
    use FieldType::*;
    let reason = match &error {
        Error::InvalidDataType => match field.field_type {
            Logical => "Not a valid logical value. ",
            Money => "The specified value is not a valid numeric expression. ",
            _ => "Expected a numeric value. ",
        },
        Error::DataTruncated => match field.field_type {
            Raw | VarBinary => "",
            _ => "Character value too long. ",
        },
        Error::InvalidFieldType
        | Error::InvalidValue
        | Error::ValueOverflow
        | Error::InvalidMonth
        | Error::InvalidData => "",
        _ => return error,
    };
    error.detailed(format!("{reason}Field: \"{}\"", field.name))
}

/// What a field takes of a default, minimum or maximum whose text is `text`
/// and whose value as an expression is `value` (`None` if it is none, or
/// fails), as the original engine reads them (probed,
/// `tools/ace/dd/rules.ops`): text goes to any field as `AdsSetField` sets it;
/// a number to a numeric field, a logical value to a logical field, a date to
/// a Date or TimeStamp field, a timestamp to a TimeStamp field; NULL (an
/// empty field the expression read) as empty text. Any other value (`NOW()`
/// for a Date or Time field among them), and a value that is no text for a
/// character field, leaves the text itself.
fn dictionary_value(field: &Field, text: &str, value: Option<Val>) -> SetValue {
    value
        .and_then(|value| taken(field, value))
        .unwrap_or_else(|| SetValue::Field(text.to_owned()))
}

/// What `field` takes of `value`, an expression's value (see
/// [`dictionary_value`]); `None` if it takes the text instead.
fn taken(field: &Field, value: Val) -> Option<SetValue> {
    use FieldType::*;
    let numeric = matches!(
        field.field_type,
        ShortInt | Integer | LongInt | Numeric | Double | CurDouble | Money
    );
    let moment = matches!(field.field_type, TimeStamp | ModTime);
    Some(match value {
        Val::Str(bytes) => SetValue::Field(tflip_storage::cp1252::decode(&bytes)),
        Val::Null => SetValue::Field(String::new()),
        _ if is_character(field) => return None,
        Val::Number(n) if numeric => SetValue::Field(number_text(n)),
        Val::Logical(b) if field.field_type == Logical => SetValue::Logical(b),
        Val::Date(day) if field.field_type == Date => SetValue::Julian(day),
        Val::Date(day) if moment => SetValue::TimeStampRaw(day, 0),
        Val::TimeStamp(day, ms) if moment => SetValue::TimeStampRaw(day, ms),
        _ => return None,
    })
}

/// Whether `field` holds text.
fn is_character(field: &Field) -> bool {
    use FieldType::*;
    matches!(
        field.field_type,
        Character | CiCharacter | VarChar | NChar | NVarChar
    )
}

/// A number as text a numeric field reads back to it: whole numbers without
/// a fraction.
fn number_text(n: f64) -> String {
    match n.fract() == 0.0 && n.abs() < 1e15 {
        true => format!("{}", n as i64),
        false => format!("{n}"),
    }
}

/// A field's value as it compares with its minimum and maximum.
#[derive(Debug, Clone, PartialEq)]
enum Key {
    Number(f64),
    Text(String),
    Moment(i64, i64),
    Bytes(Vec<u8>),
}

fn key(field: &Field, record: &[u8]) -> Option<Key> {
    Some(match field.value(record, None).ok()? {
        Value::Null | Value::Blank => return None,
        Value::Text(text) => Key::Text(text.trim_end().to_owned()),
        Value::Numeric(text) => Key::Number(text.trim().parse().ok()?),
        Value::Integer(n) => Key::Number(n as f64),
        Value::Double(d) => Key::Number(d),
        Value::Money(m) => Key::Number(m as f64 / 10_000.0),
        Value::Logical(b) => Key::Number(f64::from(u8::from(b))),
        Value::Date(day) => Key::Moment(i64::from(day), 0),
        Value::Time(ms) => Key::Moment(0, i64::from(ms)),
        Value::TimeStamp(day, ms) => Key::Moment(i64::from(day), i64::from(ms)),
        Value::Binary(bytes) => Key::Bytes(bytes),
        Value::Guid(bytes) => Key::Bytes(bytes.to_vec()),
    })
}

fn compare(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Number(a), Key::Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Key::Text(a), Key::Text(b)) => {
            tflip_storage::cp1252::encode(a).cmp(&tflip_storage::cp1252::encode(b))
        }
        (Key::Moment(a, x), Key::Moment(b, y)) => (a, x).cmp(&(b, y)),
        (Key::Bytes(a), Key::Bytes(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}
