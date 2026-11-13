//! Creating, closing, listing and configuring statements, and preparing them.

use super::*;

/// The counter statements are numbered after.
static STATEMENTS: AtomicU32 = AtomicU32::new(1);

/// Blanks every character of the SQL `text` but its parameter markers, as
/// the original engine does for `AdsGetSQLStmtParams`.
pub fn blank_all_but_params(text: &mut [u8]) {
    tflip_sql::params::blank_all_but_params(text);
}

/// Creates a statement on `connection`.
pub fn create(session: &Session, connection: Handle) -> Result<Handle, Error> {
    connection::get(session, connection)?;
    let number = STATEMENTS.fetch_add(1, atomic::Ordering::Relaxed);
    let statement = Statement {
        connection,
        number,
        name: format!("Q{:08X}_{}", number + 2, random_name()),
        state: Mutex::new(State::default()),
        last: Arc::default(),
    };
    Ok(session.insert(Object::Statement(Arc::new(statement)), Some(connection)))
}

/// 22 random characters of the URL-safe base 64 alphabet, as a statement's
/// name ends.
fn random_name() -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let id = tflip_expr::new_id();
    let mut bits = id.iter().fold(0u128, |acc, &b| acc << 8 | u128::from(b));
    // 128 bits in 22 digits of 6; the first holds the two left over.
    let mut name = [0u8; 22];
    for digit in name.iter_mut().rev() {
        *digit = DIGITS[(bits & 63) as usize];
        bits >>= 6;
    }
    String::from_utf8_lossy(&name).into_owned()
}

/// The statement behind `handle`.
pub fn get(session: &Session, handle: Handle) -> Result<Arc<Statement>, Error> {
    session
        .with(handle, |object| match object {
            Object::Statement(statement) => Some(Arc::clone(statement)),
            _ => None,
        })
        .flatten()
        .ok_or(Error::InvalidStatement)
}

/// Closes the statement behind `handle`, and its cursors.
pub fn close(session: &Session, handle: Handle) -> Result<(), Error> {
    cursor_closed(session, handle, 0);
    super::release(session, handle);
    match session.remove_if(handle, |object| matches!(object, Object::Statement(_))) {
        true => Ok(()),
        false => Err(Error::InvalidStatement),
    }
}

/// The cursor `cursor` of the statement `handle` closed (any of them, for 0):
/// the query log gets the end time of the statement that made it.
pub(crate) fn cursor_closed(session: &Session, handle: Handle, cursor: Handle) {
    let Ok(statement) = get(session, handle) else {
        return;
    };
    let end = {
        let mut state = statement.state();
        match cursor == 0 || state.cursor == Some(cursor) {
            true => state.log_end.take(),
            false => None,
        }
    };
    if let Some(end) = end {
        crate::query_log::ended(session, statement.connection, end);
    }
}

/// The SQL prepared on the statement `handle`.
pub(crate) fn sql_of(session: &Session, handle: Handle) -> Option<String> {
    get(session, handle).ok()?.state().sql.clone()
}

/// A statement with SQL prepared, as `sp_GetSQLStatements` lists it.
#[derive(Debug, Clone)]
pub(crate) struct Listed {
    pub number: u32,
    pub connection: Handle,
    pub sql: String,
    pub started: u32,
}

/// The statements of `session` that have run SQL, but `except`, in the
/// order they were created, with the SQL that last ran: as in the original
/// engine, preparing other SQL does not change it until that runs.
pub(crate) fn listed(session: &Session, except: Handle) -> Vec<Listed> {
    let mut statements = Vec::new();
    session.for_each(|object| {
        if let Object::Statement(statement) = object {
            statements.push(Arc::clone(statement));
        }
    });
    let current = get(session, except).ok().map(|s| s.number);
    statements
        .into_iter()
        .filter(|s| Some(s.number) != current)
        .filter_map(|s| {
            let (sql, started) = s.state().executed.clone()?;
            Some(Listed {
                number: s.number,
                connection: s.connection,
                sql,
                started,
            })
        })
        .collect()
}

/// What the original's server lists for `except`'s connection: the statements
/// of every client's connection to the same dictionary (or folder of free
/// tables), by number. Each comes with the session it runs in.
pub(crate) fn listed_on_server(session: &Session, except: Handle) -> Vec<(Session, Listed)> {
    let Some(asker) = get(session, except)
        .ok()
        .and_then(|s| connection::get(session, s.connection).ok())
        .map(|c| (c.path.clone(), c.dictionary))
    else {
        return Vec::new();
    };
    let current = get(session, except).ok().map(|s| s.number);
    let mut all = Vec::new();
    for other in Session::served() {
        for listed in listed(&other, 0) {
            let same = connection::get(&other, listed.connection)
                .is_ok_and(|c| (c.path.clone(), c.dictionary) == asker);
            if same && Some(listed.number) != current {
                all.push((other.clone(), listed));
            }
        }
    }
    all.sort_by_key(|(_, listed)| listed.number);
    all
}

/// Changes a setting of the tables the statement `handle` opens. As in the
/// original engine, the table and character types, whether updates are
/// constrained and whether all columns are read do not change while the last
/// execution's cursor is open ([`Error::CursorNotClosed`]); whether it is
/// read-only does (the others were not probed).
pub fn set(session: &Session, handle: Handle, setting: Setting) -> Result<(), Error> {
    let statement = get(session, handle)?;
    if matches!(
        setting,
        Setting::TableType(_)
            | Setting::CharType(_)
            | Setting::Constrain(_)
            | Setting::ReadAllColumns(_)
    ) {
        statement.check_cursor(session)?;
    }
    let mut state = statement.state();
    if setting == Setting::ParseOnly {
        state.parse_only_armed = true;
        return Ok(());
    }
    let settings = &mut state.settings;
    match setting {
        Setting::TableType(t) => settings.table_type = t,
        Setting::CharType(c) => settings.char_type = c,
        Setting::LockType(l) => settings.lock_type = l,
        Setting::ReadOnly(r) => settings.read_only = r,
        Setting::Constrain(c) => settings.constrain = c,
        Setting::ReadAllColumns(_) => {}
        Setting::Collation(c) => settings.collation = c,
        Setting::Rights(r) => settings.rights = r,
        Setting::TablePassword(table, password) => {
            let table = table.to_ascii_lowercase();
            settings.passwords.retain(|(name, _)| *name != table);
            settings.passwords.push((table, password));
        }
        Setting::ClearTablePasswords => settings.passwords.clear(),
        Setting::Encryption(password) => settings.encryption = password,
        Setting::ParseOnly => unreachable!("handled above"),
    }
    Ok(())
}

/// The settings of the tables the statement `handle` opens.
pub(crate) fn settings_of(session: &Session, handle: Handle) -> Result<Settings, Error> {
    Ok(get(session, handle)?.state().settings.clone())
}

/// Prepares `sql` on the statement `handle`; it runs when executed.
pub fn prepare(session: &Session, handle: Handle, sql: &str) -> Result<(), Error> {
    let statement = get(session, handle)?;
    statement.check_cursor(session)?;
    if sql.trim().is_empty() {
        return Err(Error::EmptyStatement);
    }
    // Errors in the text are reported when it runs, as in the original
    // engine; the parameters are known from its markers either way, one for
    // each (`fixtures/sql/params5.json`).
    let params = tflip_sql::params::markers(sql);
    let mut state = statement.state();
    // The values given to the parameters stay, by position, over any text
    // prepared after, until `AdsClearSQLParams` (`fixtures/sql/params.json`).
    let mut values: Vec<Option<ParamValue>> = std::mem::take(&mut state.params)
        .into_iter()
        .map(|param| param.value)
        .collect();
    values.append(&mut state.spare);
    values.resize(values.len().max(params.len()), None);
    state.spare = values.split_off(params.len());
    state.sql = Some(sql.to_owned());
    state.params = params
        .into_iter()
        .zip(values)
        .map(|(name, value)| Param { name, value })
        .collect();
    Ok(())
}

/// Checks `sql` on the statement `handle` without running it: its syntax,
/// tables and columns. After [`Setting::ParseOnly`] the syntax alone: for
/// that verification and, as in the original engine, for the same text
/// verified again straight after (`tools/ace/sql/props39.txt`).
pub fn verify(session: &Session, handle: Handle, sql: &str) -> Result<(), Error> {
    let statement = get(session, handle)?;
    statement.check_cursor(session)?;
    let (parse_only, settings) = {
        let mut state = statement.state();
        let parse_only = std::mem::take(&mut state.parse_only_armed)
            || state.parse_only_text.as_deref() == Some(sql);
        state.parse_only_text = None;
        (parse_only, state.settings.clone())
    };
    if sql.trim().is_empty() {
        return Err(Error::EmptyStatement);
    }
    let parsed = tflip_sql::parse(sql).map_err(Error::Sql)?;
    if parse_only {
        statement.state().parse_only_text = Some(sql.to_owned());
        return Ok(());
    }
    check(
        session,
        statement.connection,
        &settings,
        &parsed.statement,
        parsed.params.len(),
    )
}

/// Checks `statement`, which has `params` parameters, on `connection`.
pub(super) fn check(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    statement: &Sql,
    params: usize,
) -> Result<(), Error> {
    match statement {
        Sql::Select(select) => match planned_select(session, connection, settings, select, params)?
        {
            Some(_) => {}
            None => {
                let tables = Opened::open(
                    session,
                    connection,
                    settings,
                    select,
                    Procedures::Describe(params),
                )?;
                tflip_sql::query::prepare(select, &tables, params).map_err(|e| tables.error(e))?;
            }
        },
        Sql::Execute(execute) => check_execute(session, connection, execute)?,
        Sql::Debug(..) | Sql::CachePrepare(_) => {}
        Sql::ShowPlan(inner, _) => check(session, connection, settings, inner, params)?,
        // An external procedure's results, which it writes as it runs.
        Sql::Insert(insert) if crate::aep::collects(&insert.table.name) => {}
        Sql::Update(update) if crate::trigger::writes_new(&update.table.name) => {}
        Sql::Insert(_) | Sql::Update(_) | Sql::Delete(_) | Sql::Merge(_) => {
            writable(statement)?;
            let tables = Opened::open_statement(
                session,
                connection,
                settings,
                statement,
                Procedures::Describe(params),
            )?;
            tflip_sql::dml::prepare(statement, &tables, params).map_err(|e| tables.error(e))?;
        }
        // Checked as they run.
        Sql::CreateTable(_)
        | Sql::CreateTrigger(_)
        | Sql::DropTrigger(..)
        | Sql::CreateView(_)
        | Sql::DropView(_)
        | Sql::CreateDatabase(_)
        | Sql::CreateProcedure(_)
        | Sql::DropProcedure(_)
        | Sql::FunctionDdl(_)
        | Sql::CreateIndex(_)
        | Sql::DropTable(_)
        | Sql::DropIndex(..)
        | Sql::AlterTable(_)
        | Sql::Transaction(_)
        | Sql::SetVariable(_)
        | Sql::Grant(_)
        | Sql::Script(_) => {}
    }
    Ok(())
}

/// Checks a view's `SELECT`, which has `params` parameters, as a statement
/// of the connection's default settings would be.
pub(crate) fn check_select(
    session: &Session,
    connection: Handle,
    select: &tflip_sql::ast::Select,
    params: usize,
) -> Result<(), Error> {
    let select = Sql::Select(Box::new(select.clone()));
    check(session, connection, &Settings::default(), &select, params)
}

/// Checks that the procedure `execute` calls exists: a built-in system
/// procedure, or one of the connection's dictionary's.
pub(super) fn check_execute(
    session: &Session,
    connection: Handle,
    execute: &Execute,
) -> Result<(), Error> {
    if procedure::is_builtin(&execute.name) {
        return Ok(());
    }
    if !crate::psm::exists(session, connection, &execute.name)? {
        return Err(Error::Sql(SqlError::no_procedure(
            &execute.name,
            execute.pos,
        )));
    }
    refuse_procedure(session, connection, execute)
}

/// Refuses the dictionary's procedure `execute` runs (5054) where the
/// original does, before it looks at the arguments: a `DB:Public` rights slot
/// that does not check, or the right to execute it not held
/// (`fixtures/sql/ddpublicbad3`, `ddexecrights`).
pub(super) fn refuse_procedure(
    session: &Session,
    connection: Handle,
    execute: &Execute,
) -> Result<(), Error> {
    use crate::dictionary::ExecuteRefusal;
    let refusal = connection::get(session, connection)?.with_database_opt(|db| {
        let object = (db.dictionary()).named(tflip_storage::dd::Kind::Procedure, &execute.name)?;
        db.execute_refusal(object)
    });
    match refusal.flatten() {
        Some(ExecuteRefusal::BadSlot) => Err(crate::ace_error::sql_bare(5054)),
        Some(ExecuteRefusal::NotHeld) => Err(crate::ace_error::sql(5054, &execute.name)),
        None => Ok(()),
    }
}

/// The SQL prepared on the statement `handle` (`AdsGetSQLStatement`); empty
/// if none is.
pub fn text(session: &Session, handle: Handle) -> Result<String, Error> {
    Ok(get(session, handle)?
        .state()
        .sql
        .clone()
        .unwrap_or_default())
}

/// What `AdsGetRecordCount` reports for the statement `handle`.
pub fn affected(session: &Session, handle: Handle) -> Result<u32, Error> {
    Ok(get(session, handle)?.state().affected)
}

/// The cursor of the statement `handle`'s last execution, if it is still
/// open.
pub fn open_cursor(session: &Session, handle: Handle) -> Result<Option<Handle>, Error> {
    let cursor = get(session, handle)?.state().cursor;
    Ok(cursor.filter(|&cursor| session.with(cursor, |_| ()).is_some()))
}

/// The statement whose cursor `cursor` is (`AdsGetSQLStatementHandle`).
pub fn of_cursor(session: &Session, cursor: Handle) -> Result<Handle, Error> {
    let table = table::get(session, cursor).map_err(|_| Error::NoCursorHandle)?;
    table.statement().ok_or(Error::NoCursorHandle)
}

/// Whether `cursor` is a live cursor (`AdsSqlPeekStatement`): the table a
/// `SELECT` reads, rather than a table of its results. Any other handle is
/// [`Error::InvalidHandle`], as in the original engine.
pub fn is_live(session: &Session, cursor: Handle) -> Result<bool, Error> {
    let table = table::get(session, cursor).map_err(|_| Error::InvalidHandle)?;
    match table.statement() {
        Some(_) => Ok(!table.is_built()),
        None => Err(Error::InvalidHandle),
    }
}

/// Sets a parameter of the SQL prepared on the statement `handle`.
pub fn set_param(
    session: &Session,
    handle: Handle,
    param: ParamRef,
    value: ParamValue,
) -> Result<(), Error> {
    let statement = get(session, handle)?;
    let mut state = statement.state();
    let index = match &param {
        // The numbers go as far as the most parameters any text prepared
        // had, though the text now prepared has fewer
        // (`fixtures/sql/params.json`).
        ParamRef::Number(n) => n
            .checked_sub(1)
            .filter(|&i| i < state.params.len() + state.spare.len())
            .ok_or(Error::InvalidParamNumber)?,
        ParamRef::Name(name) => {
            // A name that comes twice in the text is two parameters, which
            // the name sets both of, and which keep the value by position
            // in a text prepared later (`fixtures/sql/params4.json`).
            let named = |p: &Param| {
                p.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            };
            if !state.params.iter().any(named) {
                return Err(Error::InvalidParamName);
            }
            for param in state.params.iter_mut().filter(|p| named(p)) {
                param.value = Some(value.clone());
            }
            return Ok(());
        }
    };
    match index.checked_sub(state.params.len()) {
        Some(beyond) => state.spare[beyond] = Some(value),
        None => state.params[index].value = Some(value),
    }
    Ok(())
}

/// How many parameters numbers the statement takes: the most any text
/// prepared on it had, which `AdsClearSQLParams` does not lower
/// (`fixtures/sql/params5.json`).
pub fn param_total(session: &Session, handle: Handle) -> Result<u32, Error> {
    let state = get(session, handle)?;
    let state = state.state();
    Ok((state.params.len() + state.spare.len()) as u32)
}

/// Clears the values of the statement's parameters.
pub fn clear_params(session: &Session, handle: Handle) -> Result<(), Error> {
    let statement = get(session, handle)?;
    let mut state = statement.state();
    state.spare.fill(None);
    for param in &mut state.params {
        param.value = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// As in the original engine: `sp_GetSQLStatements` lists the
    /// statements that have run, with the SQL that last ran, until they
    /// are closed.
    #[test]
    fn statements_are_listed_once_they_run() {
        let session = Session::new();
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/indexes");
        let connection =
            connection::connect(&session, &format!("Data Source={};", data.display())).unwrap();
        let [prepared, ran, failed] = [(); 3].map(|()| create(&session, connection).unwrap());
        let numbers: Vec<u32> = [prepared, ran, failed]
            .iter()
            .map(|&h| get(&session, h).unwrap().number)
            .collect();
        let sql = |handle| -> Vec<String> {
            let number = numbers[[prepared, ran, failed]
                .iter()
                .position(|&h| h == handle)
                .unwrap()];
            let listed = listed(&session, 0)
                .into_iter()
                .filter(|l| l.number == number);
            listed.map(|l| l.sql).collect()
        };

        prepare(&session, prepared, "SELECT * FROM indexed").unwrap();
        assert_eq!(sql(prepared), [""; 0]);

        let cursor = execute_direct(&session, ran, "SELECT * FROM indexed WHERE id > 2", true);
        crate::table::close(&session, cursor.unwrap().unwrap()).unwrap();
        prepare(&session, ran, "SELECT * FROM indexed WHERE id > 4").unwrap();
        assert_eq!(sql(ran), ["SELECT * FROM indexed WHERE id > 2"]);

        let failure = execute_direct(&session, failed, "SELECT nosuch FROM indexed", true);
        assert!(failure.is_err());
        assert_eq!(sql(failed), ["SELECT nosuch FROM indexed"]);

        close(&session, ran).unwrap();
        assert_eq!(sql(ran), [""; 0]);
    }
}
