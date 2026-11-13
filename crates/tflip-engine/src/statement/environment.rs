//! The environment an SQL statement is compiled and run in.

use super::*;

/// Checks the collation `name`, `[ANSI_OEM][:UNICODE]`, as `SET` does:
/// `default`, `default_ansi` and `default_oem`, or a name of the repository
/// (`adscollate.adt`, where the files are reachable), regardless of case and
/// of blanks around it, then the locale after a colon, which may be empty.
pub(super) fn check_collation(session: &Session, name: &str) -> Result<(), Error> {
    let (ansi, unicode) = match name.split_once(':') {
        Some((ansi, unicode)) => (ansi.trim(), Some(unicode.trim())),
        None => (name.trim(), None),
    };
    let load_failed = || {
        Error::Sql(SqlError::ace(
            9124,
            "An internal error was encountered.",
            "Failed to load collation: ",
        ))
    };
    if ansi.is_empty() {
        return Err(load_failed());
    }
    let collations = session.settings.collations();
    let default = ["default", "default_ansi", "default_oem"]
        .iter()
        .any(|word| ansi.eq_ignore_ascii_case(word));
    if !default && collations.has_repository() && collations.canonical(ansi).is_none() {
        return Err(Error::Sql(SqlError::ace(
            7147,
            "The collation name could not be found in the repository.",
            &format!("Failed to load collation: {ansi}"),
        )));
    }
    match unicode {
        Some(locale)
            if !locale.is_empty() && !tflip_storage::collation::Dynamic::is_icu_locale(locale) =>
        {
            Err(Error::Sql(SqlError::ace_bare(
                1564,
                &format!(
                    "Failed to open a collator for the specified locale.Failed to open Unicode \
                     collator for locale: {locale}"
                ),
            )))
        }
        _ => Ok(()),
    }
}

/// What expressions run with: the connection's date format, the time now,
/// the parameters as their types need them and the collation of the tables
/// `settings` opens (the connection's without). A parameter without a
/// value is "Parameter missing".
pub(super) fn env(
    session: &Session,
    connection: Handle,
    settings: Option<&Settings>,
    params: &[Param],
    types: &[Ty],
) -> Result<Env, Error> {
    let owner = connection::get(session, connection)?;
    let date_format = owner.date_format();
    let epoch = owner.epoch();
    let mut values = Vec::with_capacity(params.len());
    for (param, ty) in params.iter().zip(types) {
        let value = param
            .value
            .as_ref()
            .ok_or_else(|| Error::Sql(SqlError::parameter_missing()))?;
        values.push(param_value(value, *ty)?);
    }
    let now = crate::clock::now();
    let table_collation = settings.map_or("", |s| s.collation.as_str());
    let collation = collation(session, &owner, table_collation);
    Ok(Env {
        date_format,
        epoch,
        now: now.local,
        utc_now: now.utc,
        params: values,
        functions: crate::udf::functions(session, connection, settings),
        case_fold: collation.as_ref().map(|c| c.case_fold()),
        collation,
        database: owner.path.display().to_string(),
        rand: Arc::clone(&owner.rand),
        application_id: lock(&owner.application_id).clone(),
        connection_autoinc: Arc::clone(&owner.last_autoinc),
        statement_last: last(),
        xbase_blanks: false,
        engine: false,
    })
}

/// The user `USER` / `USER()` folds to for `connection`: the dictionary
/// login user, or — for a free-table connection with no login — the OS user
/// the server runs as, as the original engine reports (`%USERNAME%`, else
/// `$USER`).
pub(crate) fn sql_user(session: &Session, connection: Handle) -> Option<String> {
    let owner = connection::get(session, connection).ok()?;
    Some(
        owner
            .user()
            .map(str::to_owned)
            .unwrap_or_else(connection::os_user),
    )
}

/// The collation SQL compares text by (and `UPPER`/`LOWER` fold case by) on
/// `owner`, in a statement whose tables open with `table_collation`: that
/// dynamic collation, else the connection's (`AdsSetCollation`), as the
/// original engine compares all of the statement's text by it (probed,
/// `fixtures/sql/collation_dynamic.json`); without one (or one the
/// collation files lack), the static ANSI one (OEM tables too, as the
/// environment is the connection's); `None` under the default collation,
/// where text compares byte-wise.
pub(crate) fn collation(
    session: &Session,
    owner: &connection::Connection,
    table_collation: &str,
) -> Option<Arc<tflip_sql::Collation>> {
    let collations = session.settings.collations();
    let name = effective_collation(owner, table_collation);
    if let Some(dynamic) = (!name.is_empty())
        .then(|| collations.dynamic(&name))
        .flatten()
    {
        return Some(Arc::new(tflip_sql::Collation::Dynamic(dynamic)));
    }
    let ansi = collations.ansi()?.clone();
    Some(Arc::new(tflip_sql::Collation::Static(Box::new(ansi))))
}

/// The collation a statement's tables open with: `table_collation`
/// (`AdsStmtSetTableCollation`), else `owner`'s (`AdsSetCollation`).
pub(super) fn effective_collation(owner: &connection::Connection, table_collation: &str) -> String {
    match table_collation.trim() {
        "" => owner.collation().trim().to_owned(),
        name => name.to_owned(),
    }
}

/// The evaluation environment of `connection` for the expressions of a
/// script sent as a statement, with its parameters; one without a value is
/// NULL (its statements say it is missing when they run).
pub(crate) fn script_env(
    session: &Session,
    connection: Handle,
    params: &[Param],
) -> Result<Env, Error> {
    let filled: Vec<Param> = params
        .iter()
        .map(|param| Param {
            name: param.name.clone(),
            value: Some(param.value.clone().unwrap_or(ParamValue::Null)),
        })
        .collect();
    env(
        session,
        connection,
        None,
        &filled,
        &vec![Ty::Unknown; filled.len()],
    )
}

/// The error of the table `name`, which does not exist, as a statement of
/// `handle` opens it.
pub(crate) fn missing_table(session: &Session, handle: Handle, name: &str) -> Error {
    let found = get(session, handle).and_then(|statement| {
        let settings = statement.state().settings.clone();
        let owner = connection::get(session, statement.connection)?;
        Ok(owner
            .folder()
            .join(name)
            .with_extension(settings.extension()))
    });
    match found {
        Ok(path) => Error::Sql(SqlError::ace(
            5004,
            "Either ACE could not find the specified file, or you do not have sufficient \
             rights to access the file.",
            &format!("{} Table name: {name}", path.display()),
        )),
        Err(error) => error,
    }
}

/// The evaluation environment of `connection` with no parameters, for the
/// expressions a script runs.
pub(crate) fn connection_env(session: &Session, connection: Handle) -> Result<Env, Error> {
    env(session, connection, None, &[], &[])
}

/// A parameter's value as type `ty`.
fn param_value(value: &ParamValue, ty: Ty) -> Result<V, Error> {
    let v = match value {
        ParamValue::Null | ParamValue::Empty => return Ok(V::Null),
        ParamValue::Text(s) => V::Str(s.clone()),
        ParamValue::Integer(n) => V::Int(*n),
        ParamValue::Double(d) => V::Double(*d),
        ParamValue::Logical(b) => V::Bool(*b),
        ParamValue::Money(m) => V::Money(*m),
        ParamValue::Date(d) => V::Date(*d),
        ParamValue::TimeStamp(d, t) => V::TimeStamp(*d, *t),
        ParamValue::Binary(b) => V::Bin(b.clone()),
        // The original keeps the text of a date and reads it where the
        // parameter is used, as it does a string's
        // (`fixtures/sql/params3.json`).
        ParamValue::DateText(text) if text.is_empty() => return Ok(V::Null),
        ParamValue::DateText(text) => V::Str(text.clone()),
    };
    // Unlike other text, text for a GUID is read when the statement runs,
    // and text that is none fails it (`fixtures/sql/guid.json`).
    if let (V::Str(s), Ty::Guid) = (&v, ty) {
        return crate::setters::guid(s).map(V::Guid).map_err(|_| {
            Error::Sql(SqlError::ace_bare(
                crate::ace_error::codes::AE_INVALID_DATA,
                "The supplied data is not valid.",
            ))
        });
    }
    Ok(convert(v, ty))
}

/// A value as type `ty`, as far as it converts; else as it is.
fn convert(v: V, ty: Ty) -> V {
    // Text or a double that a NUMERIC does not hold as it is, for its
    // decimals, its digits or its form, stays as given, to be cut or refused
    // where it is read or written (`fixtures/sql/gram_assign.json`).
    if let Ty::Num { precision, scale } = ty {
        let holds = match &v {
            V::Str(s) => {
                let text = s.trim();
                !text.is_empty()
                    && tflip_sql::value::parse_exact(text, scale)
                        .is_some_and(|n| n.unsigned_abs() < 10u128.pow(u32::from(precision)))
            }
            V::Double(d) => {
                d.is_finite()
                    && d.abs() * 10f64.powi(i32::from(scale)) < 10f64.powi(i32::from(precision))
            }
            _ => true,
        };
        if !holds || tflip_sql::value::cut_decimals(&v, scale).is_some() {
            return v;
        }
    }
    match (&v, ty) {
        (V::Str(s), Ty::Int { .. }) => s.trim().parse().map_or(v, V::Int),
        (V::Str(s), Ty::Num { scale, .. }) => {
            tflip_sql::value::parse_exact(s.trim(), scale).map_or(v, |n| V::Num(n, scale))
        }
        (V::Str(s), Ty::Double | Ty::CurDouble) => s.trim().parse().map_or(v, V::Double),
        // Only 1 is true, and text that is no number stays text, which
        // the statement refuses (`fixtures/sql/params3.json`).
        (V::Str(s), Ty::Bool) => s.trim().parse::<i64>().map_or(v, |n| V::Bool(n == 1)),
        (V::Double(d), Ty::Int { .. }) => V::Int(d.trunc() as i64),
        (V::Double(d), Ty::Num { scale, .. }) => {
            V::Num((d * 10f64.powi(i32::from(scale))).round() as i128, scale)
        }
        (V::Double(d), Ty::Money) => V::Money((d * 10_000.0).round() as i64),
        (V::Int(n), Ty::Double | Ty::CurDouble) => V::Double(*n as f64),
        (V::Int(n), Ty::Num { scale, .. }) => {
            V::Num(i128::from(*n) * 10i128.pow(u32::from(scale)), scale)
        }
        (V::Int(n), Ty::Money) => V::Money(n * 10_000),
        (V::Int(n), Ty::Str { .. }) => V::Str(n.to_string()),
        (V::Double(d), Ty::Str { .. }) => V::Str(d.to_string()),
        (V::Bool(b), Ty::Int { .. }) => V::Int(i64::from(*b)),
        // Only 1 is true (`fixtures/sql/params3.json`).
        (V::Int(n), Ty::Bool) => V::Bool(*n == 1),
        (V::Double(d), Ty::Bool) => V::Bool(*d != 0.0),
        (V::Money(m), Ty::Double) => V::Double(*m as f64 / 10_000.0),
        (V::Date(d), Ty::TimeStamp) => V::TimeStamp(*d, 0),
        _ => v,
    }
}

/// The part after `system.` if `name` is a `system.*` table (not
/// `system.iota`, which is handled on its own); else `None`.
pub(super) fn system_name(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("system.").or_else(|| {
        name.get(..7)
            .filter(|p| p.eq_ignore_ascii_case("system."))
            .map(|_| &name[7..])
    })?;
    crate::system::is_system_table(rest).then_some(rest)
}

/// A table's name as the statement wrote it, without a path or extension.
pub(super) fn scope_name(from: &tflip_sql::ast::TableRef) -> &str {
    let name = from.name.rsplit(['\\', '/']).next().unwrap_or(&from.name);
    let name = crate::temp::bare(name);
    name.split('.').next().unwrap_or(name)
}
