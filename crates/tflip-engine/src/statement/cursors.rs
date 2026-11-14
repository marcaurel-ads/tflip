//! The cursors a statement opens: live, static, built and named.

use super::*;

/// The table a `SELECT` reads: a table of the connection's, `system.iota`,
/// or a `system.*` table built in memory.
pub(super) struct Source {
    pub(super) table: Option<table::Table>,
    /// Whether the table is a `system.*` table, whose cursor is always
    /// static.
    system: bool,
}

impl Source {
    fn open(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        from: &tflip_sql::ast::TableRef,
        select: Option<&Select>,
    ) -> Result<Source, Error> {
        let name = &from.name;
        if name.eq_ignore_ascii_case("system.iota") {
            return Ok(Source {
                table: None,
                system: false,
            });
        }
        // A table of a linked dictionary, opened on the connection to it.
        if let Some(link) = &from.link {
            let linked = crate::link::open(session, connection, link, name)?;
            let options = if settings.read_only {
                READ_ONLY_OPTIONS
            } else {
                LIVE_OPTIONS
            };
            let table = table::load(session, linked, name, settings.table(options))
                .map_err(|error| open_error(error, name))?;
            return Ok(Source {
                table: Some(granted(table)),
                system: false,
            });
        }
        // A trigger's `__new`/`__old`, if one is running: an in-memory table of
        // the affected record.
        if let Some((columns, rows)) = crate::trigger::lookup(name) {
            let adt = build::build(&columns, &rows).map_err(|_| Error::CorruptTable)?;
            let table = table::source(session, connection, name, static_settings(false), adt)?;
            return Ok(Source {
                table: Some(table),
                system: true,
            });
        }
        if let Some(rest) = system_name(name) {
            let connect = connection::get(session, connection)?;
            let build = |database: Option<&crate::dictionary::Database>| match select {
                Some(select) => crate::system::build_for(database, rest, select),
                None => crate::system::build(database, rest),
            };
            let built = connect
                .with_database_opt(|db| build(Some(db)))
                .unwrap_or_else(|| build(None))
                .map_err(Error::Sql)?
                .expect("a system table name");
            let adt = build::build(&built.columns, &built.rows).map_err(|_| Error::CorruptTable)?;
            let table = table::source(session, connection, name, static_settings(false), adt)?;
            return Ok(Source {
                table: Some(table),
                system: true,
            });
        }
        // A view of the connection's dictionary: the rows of its `SELECT`, in
        // a table of their own, which a statement reads as a static cursor.
        let view = connection::get(session, connection)?.with_database_opt(|db| db.view(name));
        if let Some(sql) = view.flatten() {
            if connection::get(session, connection)?
                .with_database_opt(|db| db.named_public_slot_bad(name))
                == Some(true)
            {
                return Err(open_error(Error::AgentFailed, name));
            }
            let parsed = tflip_sql::parse(&sql).map_err(Error::Sql)?;
            let Sql::Select(select) = &parsed.statement else {
                return Err(Error::InvalidObjectName);
            };
            let (columns, rows) = select_rows(session, connection, settings, select)?;
            let adt = build::build(&columns, &rows).map_err(|_| Error::CorruptTable)?;
            let table = table::source(session, connection, name, static_settings(false), adt)?;
            return Ok(Source {
                table: Some(table),
                system: true,
            });
        }
        let options = if settings.read_only {
            READ_ONLY_OPTIONS
        } else {
            LIVE_OPTIONS
        };
        let mut table_settings = settings.table_named(options, name);
        let owner = connection::get(session, connection)?;
        // A temporary table is the connection's own, or none.
        if crate::temp::is_temp(name) && owner.temps.find(name).is_none() {
            return Err(Error::Sql(crate::temp::not_found(name, true)));
        }
        table_settings.collation = effective_collation(&owner, &settings.collation);
        // A dictionary's `OPEN_TABLE` triggers may cancel the open.
        crate::trigger::fire_open(session, connection, name, &table_settings, false, 1)
            .map_err(|error| open_error(error, name))?;
        let opened = table::load(session, connection, name, table_settings.clone())
            .map_err(|error| open_error(error, name));
        if opened.is_ok() {
            crate::trigger::fire_open(session, connection, name, &table_settings, false, 4)
                .map_err(|error| open_error(error, name))?;
        }
        match opened {
            Ok(table) => {
                // As in the original engine: orders with a dynamic collation
                // need the statement's to be theirs (probed,
                // `fixtures/sql/collation_dynamic.json`).
                let opened = table.settings.collation.trim();
                for collation in crate::index::dynamic_collations(&table.path) {
                    if !collation.eq_ignore_ascii_case(opened) {
                        let opened = if opened.is_empty() { "(none)" } else { opened };
                        return Err(Error::Sql(SqlError::ace(
                            5209,
                            "Different collations are being used, which can result in poor \
                             query optimization.",
                            &format!(
                                "Table collation: {opened}, Index collation: {collation} \
                                 Table name: {name}"
                            ),
                        )));
                    }
                }
                // The statement holds its file open until it closes, and
                // its connection may keep it open after that
                // (`AdsCacheOpenCursors`).
                hold(session, &table);
                Ok(Source {
                    table: Some(granted(table)),
                    system: false,
                })
            }
            Err(Error::FileNotFound | Error::InvalidObjectName) => {
                let path = table_path(session, connection, settings, name)?;
                Err(Error::Sql(SqlError::ace(
                    5004,
                    "Either ACE could not find the specified file, or you do not have \
                     sufficient rights to access the file.",
                    &format!("{} Table name: {name}", path.display()),
                )))
            }
            // A file not of the statement's table type: a DBF file read as
            // an ADT table, or an ADT file as a DBF one, as the original's
            // SQL engine words it (probed, `fixtures/sql/deleted_counts.json`).
            Err(Error::CorruptTable) => {
                let path = table_path(session, connection, settings, name)?;
                let (code, text) = match settings.table_type {
                    table::TableType::Adt => (
                        7016,
                        "Corrupt table.  Make sure you are not attempting to open a DBF with \
                         an ADT table type or vice versa.",
                    ),
                    _ => (7014, "Invalid DBF table."),
                };
                Err(Error::Sql(SqlError::ace(
                    code,
                    text,
                    &format!("{} Table name: {name}", path.display()),
                )))
            }
            Err(e) => Err(e),
        }
    }

    /// The scope expressions over the table see.
    fn scope<'a>(&'a self, from: &'a tflip_sql::ast::TableRef) -> Option<tflip_sql::Scope<'a>> {
        let table = self.table.as_ref()?;
        let fields = std::borrow::Cow::Borrowed(table.fields());
        Some(tflip_sql::Scope {
            name: scope_name(from),
            alias: from.alias.as_deref(),
            fields,
            fts: std::rc::Rc::new(crate::index::fts_rules(table)),
            info: table.sql_info(),
        })
    }

    /// The scope expressions over the view `from` see, when `view` is the
    /// live plan of its `SELECT` over this table: the view's columns.
    fn view_scope<'a>(
        &'a self,
        from: &'a tflip_sql::ast::TableRef,
        view: &Plan,
    ) -> Option<tflip_sql::Scope<'a>> {
        let table = self.table.as_ref()?;
        let all = crate::index::fts_rules(table);
        let fts = view
            .columns
            .iter()
            .map(|c| c.field_index().and_then(|i| all.get(i).cloned().flatten()))
            .collect();
        Some(tflip_sql::Scope {
            name: scope_name(from),
            alias: from.alias.as_deref(),
            fields: std::borrow::Cow::Owned(tflip_sql::view_fields(view, table.fields())),
            fts: std::rc::Rc::new(fts),
            info: table.sql_info(),
        })
    }
}

/// The table a `SELECT` of one table reads, and its plan.
pub(super) struct Planned {
    pub(super) source: Source,
    pub(super) plan: Plan,
    /// The table's own name where the statement reads a view of it.
    pub(super) table_name: Option<String>,
}

/// The file of the table `name` a statement of `connection` opens with
/// `settings`, as its errors name it: in the connection's folder (its
/// dictionary's), with the statement's table type's extension if it has none.
fn table_path(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
) -> Result<std::path::PathBuf, Error> {
    let owner = connection::get(session, connection)?;
    // A dictionary connection's path is its `.add` file.
    let folder = match owner.dictionary {
        true => owner.path.parent().unwrap_or(&owner.path),
        false => &owner.path,
    };
    let mut path = crate::settings::local_join(session, folder, name);
    if path.extension().is_none() {
        path.set_extension(settings.extension());
    }
    Ok(path)
}

impl Planned {
    /// Whether the plan is a live cursor on the table.
    pub(super) fn is_live(&self) -> bool {
        self.plan.live && !self.source.system && self.source.table.is_some()
    }
}

/// How deep views of views are followed before the statement reads the
/// last as a table of rows.
const VIEW_DEPTH: usize = 16;

/// The `SELECT` of the view `from` names, if it names one of the
/// connection's dictionary.
fn view_select(
    session: &Session,
    connection: Handle,
    from: &tflip_sql::ast::TableRef,
) -> Result<Option<tflip_sql::Parsed>, Error> {
    if from.link.is_some()
        || from.name.eq_ignore_ascii_case("system.iota")
        || system_name(&from.name).is_some()
        || crate::trigger::lookup(&from.name).is_some()
    {
        return Ok(None);
    }
    let sql = connection::get(session, connection)?.with_database_opt(|db| db.view(&from.name));
    let Some(sql) = sql.flatten() else {
        return Ok(None);
    };
    if connection::get(session, connection)?
        .with_database_opt(|db| db.named_public_slot_bad(&from.name))
        == Some(true)
    {
        return Err(open_error(Error::AgentFailed, &from.name));
    }
    tflip_sql::parse(&sql).map(Some).map_err(Error::Sql)
}

/// `select`, a `SELECT` of one table or of a `SELECT` in parentheses, as a
/// live cursor on that table, if it is one: the original engine reads a
/// derived table of one table's columns through the table itself, as it
/// does a view (`fixtures/sql/derived.json`), with the conditions of both
/// as one and the column names of the outer `SELECT`. `None` for what
/// runs through its plan.
pub(super) fn planned_select(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    params: usize,
) -> Result<Option<Planned>, Error> {
    planned_select_at(session, connection, settings, select, params, 0)
}

fn planned_select_at(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    params: usize,
    depth: usize,
) -> Result<Option<Planned>, Error> {
    use tflip_sql::ast::From;
    if let Some(from) = select.table().filter(|_| select.is_simple()) {
        return planned(session, connection, settings, select, from, params, depth).map(Some);
    }
    let From::Derived {
        select: inner,
        alias,
        pos,
    } = &select.from
    else {
        return Ok(None);
    };
    // The parameters of the two would be numbered alike.
    if params > 0 || depth >= VIEW_DEPTH || !select.is_simple_over_derived() {
        return Ok(None);
    }
    // What fails here is reported by the plan the statement runs through.
    let Ok(Some(view)) = planned_select_at(session, connection, settings, inner, 0, depth + 1)
    else {
        return Ok(None);
    };
    if !view.is_live() || !view.plan.order.is_empty() {
        return Ok(None);
    }
    let as_table = tflip_sql::ast::TableRef {
        name: alias.clone().unwrap_or_default(),
        link: None,
        alias: alias.clone(),
        pos: *pos,
        alias_pos: *pos,
        id: 0,
    };
    let Some(scope) = view.source.view_scope(&as_table, &view.plan) else {
        return Ok(None);
    };
    let Ok(plan) = tflip_sql::plan_with(
        select,
        Some(scope),
        0,
        crate::udf::catalog(session, connection),
        sql_user(session, connection),
    ) else {
        return Ok(None);
    };
    if !plan.live {
        return Ok(None);
    }
    let plan = tflip_sql::through_view(plan, &view.plan);
    let table_name = view.table_name.or_else(|| base_table(inner));
    Ok(Some(Planned {
        source: view.source,
        plan,
        table_name,
    }))
}

/// The name of the table a chain of derived tables reads.
fn base_table(select: &Select) -> Option<String> {
    match &select.from {
        tflip_sql::ast::From::Table(table) => Some(table.name.clone()),
        tflip_sql::ast::From::Derived { select, .. } => base_table(select),
        _ => None,
    }
}

/// Opens the table `select`, which reads `from` alone, reads, and plans it.
/// A view of one table's columns, as a `SELECT` of that table, is planned
/// over the table itself: the original engine opens a live cursor on it
/// (`fixtures/sql/views.json`).
pub(super) fn planned(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    from: &tflip_sql::ast::TableRef,
    params: usize,
    depth: usize,
) -> Result<Planned, Error> {
    if depth < VIEW_DEPTH
        && let Some(parsed) = view_select(session, connection, from)?
        && let Sql::Select(inner) = &parsed.statement
        && let Some(inner_from) = inner.table().filter(|_| inner.is_simple())
        // A view that fails to open is read as a table of rows, which says why.
        && let Ok(view) = planned(session, connection, settings, inner, inner_from, 0, depth + 1)
        && view.is_live()
        && view.plan.order.is_empty()
        && let Some(scope) = view.source.view_scope(from, &view.plan)
    {
        let plan = tflip_sql::plan_with(
            select,
            Some(scope),
            params,
            crate::udf::catalog(session, connection),
            sql_user(session, connection),
        )
        .map_err(Error::Sql)?;
        if plan.live {
            let plan = tflip_sql::through_view(plan, &view.plan);
            let table_name = view.table_name.unwrap_or_else(|| inner_from.name.clone());
            return Ok(Planned {
                source: view.source,
                plan,
                table_name: Some(table_name),
            });
        }
    }
    let source = Source::open(session, connection, settings, from, Some(select))?;
    let plan = tflip_sql::plan_with(
        select,
        source.scope(from),
        params,
        crate::udf::catalog(session, connection),
        sql_user(session, connection),
    )
    .map_err(Error::Sql)?;
    Ok(Planned {
        source,
        plan,
        table_name: None,
    })
}

/// Runs a `SELECT` and returns its cursor (none without `want_cursor`).
pub(super) fn select_cursor(
    session: &Session,
    statement: Handle,
    connection: Handle,
    select: &Select,
    params: &[Param],
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    let settings = &settings_of(session, statement)?;
    if let Some(into) = &select.into {
        return crate::select_into::run(
            session,
            statement,
            connection,
            settings,
            select,
            into,
            params,
            want_cursor,
        );
    }
    select_cursor_in(
        session,
        statement,
        connection,
        settings,
        select,
        params,
        want_cursor,
    )
}

/// [`select_cursor`] for a statement with `settings`.
pub(crate) fn select_cursor_in(
    session: &Session,
    statement: Handle,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    params: &[Param],
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    let Some(Planned { source, plan, .. }) =
        planned_select(session, connection, settings, select, params.len())?
    else {
        return query_cursor(
            session,
            statement,
            connection,
            settings,
            select,
            params,
            want_cursor,
        );
    };
    let mut env = env(session, connection, Some(settings), params, &plan.params)?;
    oem_params(
        &mut env,
        source.table.as_ref().is_some_and(|t| t.sql_info().oem),
    );
    // A `system.*` table is read into a static cursor of the result.
    let live = plan.live && !source.system && !source.table.as_ref().is_some_and(|t| t.sql_only);
    let Source { table, system: _ } = source;
    if live && let Some(table) = table {
        tflip_sql::script::check_params(
            &tflip_sql::ast::Statement::Select(Box::new(select.clone())),
            &plan.params,
            &env,
        )
        .map_err(Error::Sql)?;
        if !want_cursor {
            return Ok(None);
        }
        return live_cursor(session, statement, table, plan, env, settings.constrain).map(Some);
    }
    // Anything else runs through its plan into a static cursor.
    drop(table);
    query_cursor(
        session,
        statement,
        connection,
        settings,
        select,
        params,
        want_cursor,
    )
}

/// The tables a `SELECT` of more than one table's records reads, by their
/// number in the statement.
pub(crate) struct Opened {
    tables: Vec<Option<Read>>,
    /// An error reading a table that is not an SQL error.
    failure: std::cell::RefCell<Option<Error>>,
    /// The user-defined functions the connection's dictionary offers.
    functions: Vec<tflip_sql::FunctionSig>,
    /// The user `USER` / `USER()` folds to.
    user: Option<String>,
}

/// A table a statement reads (none for `system.iota`), and the orders its
/// records can be looked up in.
struct Read {
    table: Option<Arc<table::Table>>,
    orders: Vec<Lookup>,
}

/// An order an SQL statement looks records up in.
struct Lookup {
    order: Arc<crate::index::Index>,
    /// The field its key is, or the first of its key's fields.
    field: usize,
    /// Every field of its key.
    fields: Vec<usize>,
    /// Whether its keys of text sort by a collation, not by their bytes.
    collated: bool,
}

impl Opened {
    pub(super) fn open(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        select: &Select,
        procedures: Procedures<'_>,
    ) -> Result<Opened, Error> {
        let mut sources = Vec::new();
        select.for_each_source(&mut |source| sources.push(source));
        Opened::open_all(
            session,
            connection,
            settings,
            sources,
            procedures,
            Some(select),
        )
    }

    /// The tables `statement` names, and the procedures it reads.
    pub(crate) fn open_statement(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        statement: &Sql,
        procedures: Procedures<'_>,
    ) -> Result<Opened, Error> {
        let mut sources = Vec::new();
        statement.for_each_table(&mut |table| sources.push(SourceRef::Table(table)));
        statement.for_each_procedure(&mut |id, execute| {
            sources.push(SourceRef::Procedure { id, execute });
        });
        Opened::open_all(session, connection, settings, sources, procedures, None)
    }

    fn open_all(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        sources: Vec<SourceRef<'_>>,
        procedures: Procedures<'_>,
        select: Option<&Select>,
    ) -> Result<Opened, Error> {
        let mut tables: Vec<Option<Read>> = Vec::new();
        // The procedures run last one first, after the tables are open
        // (`fixtures/sql/execproc.json`).
        let (calls, mut sources): (Vec<_>, Vec<_>) = sources
            .into_iter()
            .partition(|source| matches!(source, SourceRef::Procedure { .. }));
        sources.extend(calls.into_iter().rev());
        for source in sources {
            let id = match source {
                SourceRef::Table(table) => table.id,
                SourceRef::Procedure { id, .. } => id,
            };
            if tables.len() <= id {
                tables.resize_with(id + 1, || None);
            }
            let opened = match source {
                SourceRef::Table(table) => {
                    // What a statement reads of a table it reads alone
                    // decides what a rights slot that does not check takes
                    // from a system table.
                    let alone = select.filter(|s| s.table().is_some_and(|t| t.id == table.id));
                    Source::open(session, connection, settings, table, alone)?
                        .table
                        .map(Arc::new)
                }
                SourceRef::Procedure { execute, .. } => Some(Arc::new(procedure_table(
                    session, connection, settings, execute, procedures,
                )?)),
            };
            let orders = opened.as_ref().map_or_else(Vec::new, lookup_orders);
            tables[id] = Some(Read {
                table: opened,
                orders,
            });
        }
        let functions = crate::udf::catalog(session, connection);
        let user = sql_user(session, connection);
        Ok(Opened {
            tables,
            failure: std::cell::RefCell::new(None),
            functions,
            user,
        })
    }

    /// The engine's error for `e`: the failure underneath it, if any.
    pub(crate) fn error(&self, e: SqlError) -> Error {
        self.failure.borrow_mut().take().unwrap_or(Error::Sql(e))
    }

    fn read(&self, id: usize) -> Option<&Read> {
        self.tables.get(id)?.as_ref()
    }

    fn table(&self, id: usize) -> Option<&table::Table> {
        self.read(id)?.table.as_deref()
    }

    /// The SQL error for `e`, keeping a failure that is not one.
    fn failed(&self, e: Error) -> SqlError {
        match e {
            Error::Sql(e) => e,
            e => {
                *self.failure.borrow_mut() = Some(e);
                read_failed()
            }
        }
    }
}

/// The orders of `table` an SQL statement can look its records up in: ADI
/// orders whose key is one field's value, of every record (no condition,
/// not custom), of a type whose keys tflip builds from a value.
fn lookup_orders(table: &Arc<table::Table>) -> Vec<Lookup> {
    use tflip_storage::field::FieldType::*;
    if table.is_built() || table.settings.table_type != TableType::Adt {
        return Vec::new();
    }
    let mut out = Vec::new();
    for order in crate::index::private_orders(table, true) {
        let tag = &order.tag;
        let plain = order.kind == tflip_storage::order::Kind::Adi
            && tag.condition.is_empty()
            && !tag.custom()
            && !tag.full_text()
            && !tag.binary();
        let place = |field: &Field| table.fields().iter().position(|f| std::ptr::eq(f, field));
        let (field, fields) = match order.key_source(table) {
            crate::seek::KeySource::Field(field) => (field, None),
            // An order on several fields finds the records by its first,
            // as the original engine's do; a descending one is not read.
            crate::seek::KeySource::Fields(field) if !tag.descending => {
                let numbers: Option<Vec<usize>> = tag
                    .expression
                    .split(';')
                    .map(|part| part.trim().strip_prefix('F')?.parse().ok())
                    .collect();
                let places: Option<Vec<usize>> = numbers.and_then(|numbers| {
                    numbers
                        .into_iter()
                        .map(|n| {
                            table
                                .field(crate::table::FieldRef::Number(n))
                                .ok()
                                .and_then(place)
                        })
                        .collect()
                });
                match places {
                    Some(places) => (field, Some(places)),
                    None => continue,
                }
            }
            _ => continue,
        };
        let Some(number) = place(field) else {
            continue;
        };
        let typed = matches!(
            field.field_type,
            Character
                | CiCharacter
                | ShortInt
                | Integer
                | AutoInc
                | LongInt
                | Double
                | CurDouble
                | Numeric
                | Money
                | Date
                | Time
                | TimeStamp
                | ModTime
                | Logical
        );
        // A key tflip cannot build (a collation it lacks) rules it out.
        if plain && typed && table.value_key(&order, number, &Value::Null).is_ok() {
            let collated = !order.tag.collation.is_empty()
                || table.session_settings().collations().ansi().is_some();
            out.push(Lookup {
                order,
                field: number,
                fields: fields.unwrap_or_else(|| vec![number]),
                collated,
            });
        }
    }
    out
}

/// The value `field` holds that bounds a lookup by `v` from below (or with
/// `high`, above) as SQL compares them: the nearest one inside the bound
/// for numbers, of the field's precision, or a value a little outside it
/// for a DOUBLE. `None` if the field holds no value of `v`'s kind.
pub(crate) fn bound_value(field: &Field, v: &V, high: bool) -> Option<Value> {
    use tflip_storage::field::FieldType::*;
    // `v` × 10^`scale`, rounded into the bound (out of it for a double).
    let scaled = |scale: u16| -> Option<i128> {
        let (n, from) = match v {
            V::Double(d) => {
                let x = d * 10f64.powi(i32::from(scale));
                let x = if high { x.ceil() } else { x.floor() };
                return (x.is_finite() && x.abs() < 1e36).then_some(x as i128);
            }
            V::Int(n) => (i128::from(*n), 0),
            V::Num(n, s) => (*n, *s),
            V::Money(m) => (i128::from(*m), 4),
            _ => return None,
        };
        let pow = |n: u16| 10i128.checked_pow(u32::from(n));
        match from <= scale {
            true => n.checked_mul(pow(scale - from)?),
            // The largest value not above `v`, or the smallest not below.
            false => {
                let d = pow(from - scale)?;
                Some(if high {
                    n.div_euclid(d)
                } else {
                    -(-n).div_euclid(d)
                })
            }
        }
    };
    Some(match (field.field_type, v) {
        (ShortInt | Integer | AutoInc | LongInt, _) => Value::Integer(scaled(0)?),
        (Double | CurDouble, _) => {
            let d = v.as_f64().filter(|_| !matches!(v, V::Bool(_)))?;
            Value::Double(if high { d.next_up() } else { d.next_down() })
        }
        (Numeric, _) => {
            let decimals = field.decimals();
            let n = scaled(decimals)?;
            let digits = format!(
                "{:0>width$}",
                n.unsigned_abs(),
                width = usize::from(decimals) + 1
            );
            let (whole, fraction) = digits.split_at(digits.len() - usize::from(decimals));
            let sign = if n < 0 { "-" } else { "" };
            Value::Numeric(match decimals {
                0 => format!("{sign}{whole}"),
                _ => format!("{sign}{whole}.{fraction}"),
            })
        }
        (Money, _) => Value::Money(i64::try_from(scaled(4)?).ok()?),
        (Date, V::Date(d)) => Value::Date(*d),
        (Time, V::Time(t)) => Value::Time(*t),
        (TimeStamp | ModTime, V::TimeStamp(d, t)) => Value::TimeStamp(*d, *t),
        (Logical, V::Bool(b)) => Value::Logical(*b),
        (Character | CiCharacter, V::Str(s)) => Value::Text(s.clone()),
        _ => return None,
    })
}

/// The text parameters of `env` as the text of the OEM tables of the
/// statement (`oem`), as a literal is (probed, `fixtures/sql/oem.json`).
pub(super) fn oem_params(env: &mut Env, oem: bool) {
    if oem {
        for param in &mut env.params {
            if let V::Str(text) = param {
                *text = tflip_sql::compile::oem_text(text);
            }
        }
    }
}

impl Opened {
    /// Whether every table the statement reads is an OEM one.
    pub(super) fn oem(&self) -> bool {
        let mut tables = self
            .tables
            .iter()
            .flatten()
            .filter_map(|r| r.table.as_ref());
        let first = tables.next();
        first.is_some_and(|t| t.sql_info().oem) && tables.all(|t| t.sql_info().oem)
    }
}

impl tflip_sql::query::Tables for Opened {
    fn fields(&self, id: usize) -> &[Field] {
        self.table(id).map_or(&[], |table| table.fields())
    }

    fn functions(&self) -> Vec<tflip_sql::FunctionSig> {
        self.functions.clone()
    }

    fn user(&self) -> Option<String> {
        self.user.clone()
    }

    fn info(&self, id: usize) -> tflip_sql::compile::TableInfo {
        self.table(id)
            .map(table::Table::sql_info)
            .unwrap_or_default()
    }

    fn scan(
        &self,
        id: usize,
        f: &mut dyn FnMut(&mut dyn tflip_sql::Row) -> Result<(), SqlError>,
    ) -> Result<(), SqlError> {
        let Some(table) = self.table(id) else {
            return f(&mut NoRow);
        };
        let fields = table.fields();
        table
            .scan(|slot, bytes, memo| {
                f(&mut SqlRow {
                    fields,
                    bytes,
                    memo,
                    record: Some(slot),
                    hidden: table.sql_hidden(),
                })
                .map_err(Error::Sql)
            })
            .map_err(|e| self.failed(e))
    }

    fn record_count(&self, id: usize) -> Option<usize> {
        let count = self.table(id)?.record_count().ok()?;
        usize::try_from(count).ok()
    }

    fn indexes(&self, id: usize) -> Vec<tflip_sql::query::TableIndex> {
        let Some(read) = self.read(id) else {
            return Vec::new();
        };
        read.orders
            .iter()
            .map(|lookup| tflip_sql::query::TableIndex {
                name: lookup.order.tag.name.clone(),
                expression: lookup.order.expression(),
                field: lookup.field,
                fields: lookup.fields.clone(),
                ci: self
                    .table(id)
                    .map(|table| {
                        lookup
                            .fields
                            .iter()
                            .map(|&f| {
                                table.fields()[f].field_type
                                    == tflip_storage::field::FieldType::CiCharacter
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                collated: lookup.collated,
            })
            .collect()
    }

    fn fts(&self, id: usize) -> Vec<Option<tflip_sql::fts::Rules>> {
        self.read(id)
            .and_then(|read| read.table.as_ref())
            .map_or_else(Vec::new, |table| crate::index::fts_rules(table))
    }

    fn fts_lookup(
        &self,
        id: usize,
        column: Option<usize>,
        query: &tflip_sql::fts::Query,
    ) -> Result<Option<tflip_sql::query::FtsLookup>, SqlError> {
        let Some(table) = self.read(id).and_then(|read| read.table.as_ref()) else {
            return Ok(None);
        };
        let orders = crate::index::private_orders(table, true);
        fts_found(table, &orders, column, query).map_err(|e| self.failed(e))
    }

    fn lookup(
        &self,
        id: usize,
        index: usize,
        low: Option<&V>,
        high: Option<&V>,
    ) -> Result<Vec<u32>, SqlError> {
        let (Some(table), Some(lookup)) = (
            self.table(id),
            self.read(id).and_then(|read| read.orders.get(index)),
        ) else {
            return Ok(Vec::new());
        };
        let Lookup {
            order,
            field,
            fields,
            collated,
        } = lookup;
        let slot = &table.fields()[*field];
        // SQL compares text without its trailing blanks, which keys of their
        // bytes are padded with: a bound takes in every key that starts with
        // its text.
        let raw_text = slot.field_type == tflip_storage::field::FieldType::Character && !collated;
        // A key of several fields is looked up by its first: the rest of it
        // takes in every value.
        let segment = match fields.len() {
            1 => usize::MAX,
            _ => tflip_storage::key::field_key_length(slot),
        };
        let widened = |mut key: Vec<u8>, high: bool| {
            let part = segment.min(key.len());
            let end = match raw_text {
                true => key[..part]
                    .iter()
                    .rposition(|&b| b != b' ')
                    .map_or(0, |i| i + 1),
                false if segment < key.len() => segment,
                false => return key,
            };
            key[end..].fill(if high { 0xFF } else { 0 });
            key
        };
        // A bound the field cannot hold bounds nothing.
        let key = |v: Option<&V>, high: bool| match v.and_then(|v| bound_value(slot, v, high)) {
            Some(value) => Ok(table
                .value_key(order, *field, &value)?
                .map(|k| widened(k, high))),
            None => Ok(None),
        };
        let found = (|| {
            let low = key(low, false)?;
            let high = key(high, true)?;
            order.records_between(low.as_deref(), high.as_deref())
        })();
        found.map_err(|e| self.failed(e))
    }

    fn fetch(
        &self,
        id: usize,
        records: &[u32],
        f: &mut dyn FnMut(u32, &mut dyn tflip_sql::Row) -> Result<(), SqlError>,
    ) -> Result<(), SqlError> {
        let Some(table) = self.table(id) else {
            return Ok(());
        };
        let fields = table.fields();
        for &record in records {
            table
                .read_active(record, |bytes, memo| {
                    f(
                        record,
                        &mut SqlRow {
                            fields,
                            bytes,
                            memo,
                            record: Some(record),
                            hidden: table.sql_hidden(),
                        },
                    )
                    .map_err(Error::Sql)
                })
                .map_err(|e| self.failed(e))?;
        }
        Ok(())
    }
}

/// Runs a `SELECT` that is more than one table's records (joins,
/// grouping, `UNION`, subqueries) into a static cursor.
fn query_cursor(
    session: &Session,
    statement: Handle,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    params: &[Param],
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    let Some(result) = query_rows(
        session,
        statement,
        connection,
        settings,
        select,
        params,
        match want_cursor {
            true => Keep::RowsLeavingOut,
            false => Keep::Nothing,
        },
    )?
    else {
        return Ok(None);
    };
    static_cursor(
        session,
        statement,
        connection,
        result.columns,
        result.rows,
        result.sequence,
        result.temporary,
        result.oem,
        result.uncounted,
    )
    .map(Some)
}

/// What a `SELECT` that runs through its plan gives.
pub(crate) struct QueryRows {
    pub columns: Vec<Column>,
    /// The rows, in the order they were read.
    pub rows: Vec<Vec<Value>>,
    /// The 1-based numbers of the rows, in the order a cursor shows them, if
    /// that is not the order they are in.
    pub sequence: Option<Vec<u32>>,
    temporary: Option<tflip_sql::TempIndex>,
    /// Whether its tables are OEM ones, whose text its rows hold as they are.
    oem: bool,
    /// Whether rows too large for their column were left out.
    uncounted: bool,
}

/// What [`query_rows`] keeps of a `SELECT`'s result.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Keep {
    /// Nothing: the statement runs for its effects only.
    Nothing,
    /// Every row; a value too large for its column fails.
    Rows,
    /// The rows, leaving out of an unordered result one whose value is too
    /// large for its column, unless it is the first (see
    /// [`stored_rows_leaving_out`]).
    RowsLeavingOut,
}

/// Runs a `SELECT` through its plan, keeping of its rows what `keep` says.
pub(crate) fn query_rows(
    session: &Session,
    statement: Handle,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    params: &[Param],
    keep: Keep,
) -> Result<Option<QueryRows>, Error> {
    let procedures = Procedures::Run {
        statement: Some(statement),
        params,
    };
    let tables = Opened::open(session, connection, settings, select, procedures)?;
    let mut prepared =
        tflip_sql::query::prepare(select, &tables, params.len()).map_err(|e| tables.error(e))?;
    let mut env = env(
        session,
        connection,
        Some(settings),
        params,
        &prepared.params,
    )?;
    oem_params(&mut env, tables.oem());
    let output = prepared.run(&tables, &env).map_err(|e| tables.error(e))?;
    if keep == Keep::Nothing {
        return Ok(None);
    }
    let (rows, kept) = match keep == Keep::RowsLeavingOut && output.sequence.is_none() {
        true => stored_rows_leaving_out(&output.rows, &prepared.columns)?,
        false => (stored_rows(&output.rows, &prepared.columns)?, None),
    };
    let uncounted = kept.is_some();
    let columns = columns_of(&prepared.columns);
    let temporary = prepared.order_index.take().filter(|_| rows.len() > 1);
    Ok(Some(QueryRows {
        columns,
        rows,
        sequence: output.sequence.or(kept),
        temporary,
        oem: tables.oem(),
        uncounted,
    }))
}

/// Runs the DML statement `statement` (an `INSERT`, `UPDATE` or `DELETE`
/// with no parameters, as a script runs it after substituting its variables)
/// on `connection`, and returns how many rows it changed, or the error.
pub(crate) fn run_dml(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    statement: &Sql,
) -> Result<u32, Error> {
    let done = dml_statement(session, connection, settings, statement, &[]);
    done.result.map(|()| done.rows)
}

/// Runs the `SELECT` `select` with no parameters on `connection`, owned by
/// the statement `handle`, and returns its cursor (none without
/// `want_cursor`): what a script's bare `SELECT` (a `CURSOR VARYING` output)
/// returns.
pub(crate) fn run_select(
    session: &Session,
    handle: Handle,
    connection: Handle,
    select: &Select,
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    select_cursor(session, handle, connection, select, &[], want_cursor)
}

/// The columns of a query's result as the table a static cursor of it has.
fn columns_of(columns: &[tflip_sql::Column]) -> Vec<Column> {
    columns
        .iter()
        .map(|c| Column {
            name: c.name.clone(),
            field_type: c.field.0,
            length: c.field.1,
            decimals: c.field.2,
        })
        .collect()
}

/// The rows of a query's result as the values a table stores.
fn stored_rows(rows: &[Vec<V>], columns: &[tflip_sql::Column]) -> Result<Vec<Vec<Value>>, Error> {
    rows.iter()
        .map(|row| {
            row.iter()
                .zip(columns)
                .map(|(value, column)| value.to_field(column.ty, &column.name).map_err(Error::Sql))
                .collect()
        })
        .collect()
}

/// The records of a static cursor, and the 1-based numbers of the rows
/// that stay when some were left out.
type StoredRows = (Vec<Vec<Value>>, Option<Vec<u32>>);

/// [`stored_rows`] for a static cursor: the original engine fails with the
/// first row that has a value too large for its column, but leaves out a
/// later one, keeping its record number and counting no records afterwards
/// (`fixtures/sql/gram_syntax.json`). Those left out are blank records here,
/// and the second result lists the 1-based numbers of the rows that stay.
fn stored_rows_leaving_out(
    rows: &[Vec<V>],
    columns: &[tflip_sql::Column],
) -> Result<StoredRows, Error> {
    let mut stored = Vec::with_capacity(rows.len());
    let mut kept = Vec::with_capacity(rows.len());
    let mut left_out = false;
    for (i, row) in rows.iter().enumerate() {
        let converted: Result<Vec<Value>, Error> = row
            .iter()
            .zip(columns)
            .map(|(value, column)| value.to_field(column.ty, &column.name).map_err(Error::Sql))
            .collect();
        match converted {
            Ok(row) => {
                stored.push(row);
                kept.push(i as u32 + 1);
            }
            Err(e) if i == 0 => return Err(e),
            Err(_) => {
                stored.push(vec![Value::Null; columns.len()]);
                left_out = true;
            }
        }
    }
    Ok((stored, left_out.then_some(kept)))
}

/// `rows` in the order of `sequence`, the 1-based numbers of the rows in the
/// order a cursor shows them, if it is not the order they are in.
pub(crate) fn in_sequence(mut rows: Vec<Vec<Value>>, sequence: Option<&[u32]>) -> Vec<Vec<Value>> {
    match sequence {
        Some(sequence) => sequence
            .iter()
            .map(|&n| std::mem::take(&mut rows[n as usize - 1]))
            .collect(),
        None => rows,
    }
}

/// The field a column of a result is.
pub(crate) fn field_of(column: &Column) -> Field {
    tflip_storage::adt::field(
        &column.name,
        column.field_type,
        0,
        column.length,
        column.decimals,
    )
}

/// The columns and rows of the `SELECT` `select` with no parameters on
/// `connection`: what a script collects when it writes a `SELECT` to
/// `__output`.
pub(crate) fn select_rows(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &Select,
) -> Result<(Vec<Column>, Vec<Vec<Value>>), Error> {
    let tables = Opened::open(
        session,
        connection,
        settings,
        select,
        Procedures::Run {
            statement: None,
            params: &[],
        },
    )?;
    let prepared = tflip_sql::query::prepare(select, &tables, 0).map_err(|e| tables.error(e))?;
    let env = env(session, connection, Some(settings), &[], &prepared.params)?;
    let output = prepared.run(&tables, &env).map_err(|e| tables.error(e))?;
    let columns = columns_of(&prepared.columns);
    let mut rows = stored_rows(&output.rows, &prepared.columns)?;
    // What a script, a subquery or a variable reads of OEM tables is ANSI
    // text, as a static cursor gives it (`fixtures/sql/oemrows.json`); the
    // statements the script then runs meet it as client text again.
    if tables.oem() {
        for value in rows.iter_mut().flatten() {
            if let Value::Text(text) = value {
                *text = tflip_sql::compile::oem_to_ansi_text(text);
            }
        }
    }
    Ok((columns, in_sequence(rows, output.sequence.as_deref())))
}

/// The connection and the table settings of the statement `handle`.
pub(crate) fn connection_and_settings(
    session: &Session,
    handle: Handle,
) -> Result<(Handle, Settings), Error> {
    let statement = get(session, handle)?;
    let settings = statement.state().settings.clone();
    Ok((statement.connection, settings))
}

/// The column names and the rows, as values, of the `SELECT` `select` with no
/// parameters on `connection`: what a cursor of a script reads.
pub(crate) fn select_values(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &Select,
) -> Result<(Vec<Field>, Vec<Vec<V>>), Error> {
    let (columns, rows) = select_rows(session, connection, settings, select)?;
    let fields: Vec<Field> = columns.iter().map(field_of).collect();
    let types: Vec<Ty> = fields.iter().map(Ty::of_field).collect();
    let rows = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .zip(&types)
                .map(|(value, ty)| V::from_field(value, *ty))
                .collect()
        })
        .collect();
    Ok((fields, rows))
}

/// Builds a static cursor of `statement` with `columns` and `rows`: what a
/// script returns from the rows it wrote to `__output`.
pub(crate) fn build_cursor(
    session: &Session,
    statement: Handle,
    connection: Handle,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
) -> Result<Handle, Error> {
    static_cursor(
        session, statement, connection, columns, rows, None, None, false, false,
    )
}

/// The settings of an in-memory table a statement reads as a static cursor;
/// `oem` if its text is in the client's OEM code page.
pub(super) fn static_settings(oem: bool) -> TableSettings {
    TableSettings {
        table_type: TableType::Adt,
        char_type: if oem { 2 } else { 1 },
        oem_text: oem,
        options: STATIC_OPTIONS,
        ..TableSettings::default()
    }
}

/// The counter static cursors are named after (`tim_<n>.tmp`), and
/// temporary indexes (`TIM_<n>`).
static STATIC_CURSORS: AtomicU32 = AtomicU32::new(1);

/// The number the next temporary file is named after.
pub(crate) fn next_temporary() -> u32 {
    STATIC_CURSORS.fetch_add(1, atomic::Ordering::Relaxed)
}

/// Builds a static cursor of `statement` with `columns` and `rows`, shown
/// in the order of `sequence` if given.
#[allow(clippy::too_many_arguments)]
pub(super) fn static_cursor(
    session: &Session,
    statement: Handle,
    connection: Handle,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    sequence: Option<Vec<u32>>,
    temporary: Option<tflip_sql::TempIndex>,
    oem: bool,
    uncounted: bool,
) -> Result<Handle, Error> {
    let n = next_temporary();
    let name = format!("tim_{n:x}.tmp");
    let order = temporary.map(|spec| (format!("TIM_{:X}", next_temporary()), spec));
    let mut cursor = built_cursor(session, statement, connection, &name, columns, rows, oem)?;
    cursor.set_sequence(sequence);
    if uncounted {
        cursor.set_uncounted();
    }
    cursor.set_cursor_order(order.as_ref().map(|(name, _)| name.clone()));
    let cursor = table::register(session, cursor, Some(statement))?;
    if let Some((name, spec)) = order {
        crate::index::open_temporary(session, cursor, &name, &spec)?;
    }
    Ok(cursor)
}

/// The counter plan cursors are named after (`timace_<n>.tmp`).
pub(super) static PLAN_CURSORS: AtomicU32 = AtomicU32::new(1);

/// Builds a static cursor of `statement` called `name`.
pub(super) fn named_cursor(
    session: &Session,
    statement: Handle,
    connection: Handle,
    name: &str,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    sequence: Option<Vec<u32>>,
) -> Result<Handle, Error> {
    let mut cursor = built_cursor(session, statement, connection, name, columns, rows, false)?;
    cursor.set_sequence(sequence);
    table::register(session, cursor, Some(statement))
}

/// The table of a static cursor of `statement` called `name`, not yet
/// registered.
fn built_cursor(
    session: &Session,
    statement: Handle,
    connection: Handle,
    name: &str,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    oem: bool,
) -> Result<table::Table, Error> {
    let table = build::build(&columns, &rows).map_err(|_| Error::CorruptTable)?;
    table::built(
        session,
        connection,
        statement,
        name,
        static_settings(oem),
        table,
    )
}

/// What a live cursor uses of its table's orders (those it opens with):
/// its `WHERE` as the original engine's AOF, and the records that AOF's
/// orders find (looked up as ranges of their keys, a superset of those the
/// `WHERE` holds for); the order whose key its `ORDER BY` is, and the
/// records in that order. With `details`, how many records each part of the
/// AOF finds, for its plan. And the `WHERE` with the `CONTAINS` parts the AOF's
/// full-text orders answer marked (see [`tflip_sql::compile::mark_answered`]).
pub(super) struct LiveUse {
    pub(super) indexes: tflip_sql::explain::LiveIndexes,
    superset: Option<crate::filter::Bits>,
    sequence: Option<Vec<u32>>,
    filter: Option<tflip_sql::Node>,
    /// The records the original engine counts for the `WHERE` (see
    /// [`fts_count`]).
    count: Option<u32>,
    /// Whether the cursor reads the order backwards, which the order's
    /// handle then does too.
    backwards: bool,
}

pub(super) fn live_use(
    table: &Arc<table::Table>,
    plan: &Plan,
    params: &[V],
    details: bool,
) -> Result<LiveUse, Error> {
    use tflip_sql::explain::{LiveAof, LiveDetail, LiveIndexes};
    let mut used = LiveUse {
        indexes: LiveIndexes::default(),
        superset: None,
        sequence: None,
        filter: plan.filter.clone(),
        count: None,
        backwards: false,
    };
    if table.is_built() {
        return Ok(used);
    }
    let orders = crate::index::private_orders(table, true);
    let aof = plan
        .filter
        .as_ref()
        .and_then(|f| tflip_sql::xbase::aof(f, table.fields(), params));
    // An AOF the original engine cannot set (a full-text condition it cannot
    // parse): its plan says so, and the records are read one by one.
    if let (Some(text), Some(filter)) = (&aof, &plan.filter)
        && tflip_sql::xbase::unsettable(filter)
    {
        let aof = LiveAof {
            text: text.clone(),
            level: 0,
            residual: String::new(),
            details: vec![],
        };
        used.indexes.aof = Some(aof);
        return Ok(used);
    }
    // The `WHERE`'s `CONTAINS` terms, which its fields' full-text orders answer.
    let fts = match &plan.filter {
        Some(filter) => fts_terms(table, filter, &orders)?,
        None => Vec::new(),
    };
    let terms: Vec<tflip_expr::optimize::FtsTerm> = fts.iter().map(|(t, _)| t.clone()).collect();
    // The original engine answers a `CONTAINS` through its order where the AOF
    // is optimized down to it, else from the record's text.
    if aof.is_some()
        && !terms.is_empty()
        && let Some(filter) = &mut used.filter
    {
        let fields = table.fields();
        tflip_sql::compile::mark_answered(filter, &mut |part| {
            let Some(text) = tflip_sql::xbase::aof(part, fields, params) else {
                return false;
            };
            table
                .optimize_with(&text, &orders, &terms)
                .is_ok_and(|(optimized, _)| optimized.level != tflip_expr::optimize::Level::None)
        });
    }
    if let Some(text) = aof
        && let Ok((mut optimized, serving)) = table.optimize_with(&text, &orders, &terms)
    {
        used.superset = table.lookup_superset(&optimized.bound, &serving)?;
        // With no candidates, the original engine reports nothing left to
        // evaluate.
        if optimized.level == tflip_expr::optimize::Level::Partial
            && used
                .superset
                .as_ref()
                .is_some_and(|found| found.count() == 0)
        {
            optimized.residual.clear();
        }
        let mut lines = Vec::new();
        // The original engine looks nothing up for an AOF it cannot
        // optimize.
        let looked_up = optimized.level != tflip_expr::optimize::Level::None;
        if details {
            // Of an `.AND.`, the original engine looks up no further once the
            // parts looked up leave no record, or once two lookups leave one
            // and what is next is not a full-text term. A part's records are
            // known once its lines are all looked up.
            use tflip_expr::optimize::Part;
            let all = |found: &crate::filter::Bits| {
                let mut out = crate::filter::Bits::default();
                (1..=table.slot_count())
                    .filter(|&r| !found.get(r))
                    .for_each(|r| out.set(r, true));
                out
            };
            let (mut lookups, mut left): (u32, Option<crate::filter::Bits>) = (0, None);
            // The part being looked up, and what its lines found so far
            // (`None` once one of them found nothing it can tell).
            let mut part: Option<(usize, Option<crate::filter::Bits>)> = None;
            let close = |part: Option<(usize, Option<crate::filter::Bits>)>,
                         left: &mut Option<crate::filter::Bits>| {
                let Some((number, Some(found))) = part else {
                    return;
                };
                let found = match optimized.parts.get(number) {
                    Some(Part::Term | Part::Or) => found,
                    Some(Part::Not) => all(&found),
                    _ => return,
                };
                *left = Some(match left.take() {
                    Some(left) => left.and(&found),
                    None => found,
                });
            };
            for detail in &optimized.details {
                if part
                    .as_ref()
                    .is_some_and(|(number, _)| *number != detail.part)
                {
                    close(part.take(), &mut left);
                }
                let current = part.get_or_insert((detail.part, Some(Default::default())));
                let left_count = left.as_ref().map(crate::filter::Bits::count);
                let done = lookups >= 1 && left_count == Some(0)
                    || lookups >= 2 && left_count.is_some_and(|n| n <= 1) && detail.fts.is_none();
                let looked_up = looked_up && detail.looked_up && !done;
                // What the line finds, and how many records it shows.
                let (found, actual, index) = match detail.fts.and_then(|i| fts.get(i)) {
                    // A full-text term: its order, and the records it finds
                    // (none found is counted, unlike an ordinary order's).
                    Some((term, order)) => {
                        let index = order
                            .clone()
                            .filter(|_| term.level != tflip_expr::optimize::Level::None);
                        match (looked_up, order) {
                            (true, Some(_)) => {
                                let found = crate::filter::Bits::of(&term.records);
                                (Some(found), Some(term.records.len() as u32), index)
                            }
                            _ => (None, None, index),
                        }
                    }
                    None => {
                        let index = detail.order.map(|order| serving[order].tag.name.clone());
                        match looked_up {
                            true => match table.detail_records(detail, &serving)? {
                                Some((count, found)) => {
                                    let found = (!detail.negated).then_some(found);
                                    (found, Some(count).filter(|&n| n > 0), index)
                                }
                                None => (None, None, index),
                            },
                            false => (None, None, index),
                        }
                    }
                };
                if actual.is_some() || found.is_some() {
                    lookups += 1;
                }
                current.1 = match (current.1.take(), found) {
                    (Some(so_far), Some(found)) => Some(so_far.or(&found)),
                    _ => None,
                };
                lines.push(LiveDetail {
                    index,
                    expr: detail.text.clone(),
                    actual,
                });
            }
            close(part.take(), &mut left);
        }
        // Full-text searches the orders answer with nothing left to
        // evaluate count what their keys find.
        if optimized.level != tflip_expr::optimize::Level::None
            && optimized.residual.is_empty()
            && let Some(filter) = &plan.filter
        {
            // An ordinary part an order finds holds for the records it finds.
            let mut other = |part: &tflip_sql::Node| -> Option<std::collections::BTreeSet<u32>> {
                let text = tflip_sql::xbase::aof(part, table.fields(), params)?;
                let (optimized, _) = table.optimize_with(&text, &orders, &[]).ok()?;
                if optimized.level != tflip_expr::optimize::Level::Full {
                    return None;
                }
                let env = tflip_sql::Env {
                    params: params.to_vec(),
                    ..Default::default()
                };
                let mut found = std::collections::BTreeSet::new();
                table
                    .scan(|slot, bytes, memo| {
                        let fields = table.fields();
                        let mut row = SqlRow {
                            fields,
                            bytes,
                            memo,
                            record: Some(slot),
                            hidden: &[],
                        };
                        if tflip_sql::eval(part, &mut row, &env).ok() == Some(V::Bool(true)) {
                            found.insert(slot);
                        }
                        Ok(())
                    })
                    .ok()?;
                Some(found)
            };
            used.count = fts_count(
                filter,
                table.fields(),
                &terms,
                table.slot_count(),
                &mut other,
            );
        }
        let (level, residual) = (optimized.level.ace(), optimized.residual);
        used.indexes.aof = Some(LiveAof {
            text,
            level,
            residual,
            details: lines,
        });
    }
    // An order whose key is the `ORDER BY`'s columns, read forward or back.
    let columns: Option<Vec<&str>> = plan
        .order
        .iter()
        .map(|key| match key.node.kind {
            tflip_sql::compile::Kind::Column(i) => table.fields().get(i).map(|f| f.name.as_str()),
            _ => None,
        })
        .collect();
    let descending = plan.order.first().map(|key| key.descending);
    let one_way = plan
        .order
        .iter()
        .all(|key| Some(key.descending) == descending);
    if let Some(columns) = columns.filter(|c| !c.is_empty() && one_way) {
        let wanted = columns.join(";").to_ascii_uppercase();
        let found = orders.iter().find(|order| {
            let tag = &order.tag;
            tag.condition.is_empty()
                && !tag.custom()
                && !tag.full_text()
                && !tag.binary()
                && order.expression().replace(' ', "").to_ascii_uppercase() == wanted
        });
        if let Some(order) = found {
            used.indexes.order = Some(order.tag.name.clone());
            let mut records = order.records_between(None, None)?;
            if descending != Some(order.tag.descending) {
                records.reverse();
                used.backwards = true;
            }
            used.sequence = Some(records);
        }
    }
    Ok(used)
}

/// Makes `table` a live cursor of `statement`.
fn live_cursor(
    session: &Session,
    statement: Handle,
    table: table::Table,
    plan: Plan,
    env: Env,
    constrain: bool,
) -> Result<Handle, Error> {
    // The table's orders hold on to it while they are open.
    let table = Arc::new(table);
    // A full-text condition that does not parse fails as the original engine
    // sets the AOF, with its own error.
    if let Some(code) = plan.filter.as_ref().and_then(invalid_fts) {
        return Err(Error::FullText(code));
    }
    if let Some(filter) = &plan.filter {
        tflip_sql::eval::check_text_constants(filter, &env).map_err(Error::Sql)?;
    }
    let used = live_use(&table, &plan, &env.params, false)?;
    let Ok(mut table) = Arc::try_unwrap(table) else {
        return Err(Error::NotImplemented);
    };
    let base: Vec<Field> = table.fields().to_vec();
    let fields = plan
        .columns
        .iter()
        .filter_map(|c| {
            let mut field = base.get(c.field_index()?)?.clone();
            field.name.clone_from(&c.name);
            Some(field)
        })
        .collect();
    let sql = used.filter.map(|condition| {
        // A condition without columns is decided now (a division by zero in
        // it fails for each record, as one with columns does), and one with
        // a literal comparison that is false reads no record.
        let columns = condition.any(&|n| {
            use tflip_sql::compile::Kind;
            matches!(
                n.kind,
                Kind::Column(_) | Kind::RecNo(_) | Kind::Deleted(_) | Kind::RowId(..)
            )
        });
        let constant = if tflip_sql::plan::folds_false(&condition) {
            Some(false)
        } else if columns {
            None
        } else {
            match tflip_sql::eval(&condition, &mut NoRow, &env) {
                Err(e) if e.native == 2153 => None,
                result => Some(result.is_ok_and(|v| v == V::Bool(true))),
            }
        };
        let superset = used
            .superset
            .clone()
            .map(|records| (table.file_changes(), records));
        let mut env = env.clone();
        env.collation = env.collation.as_ref().map(tflip_sql::Collation::for_filter);
        env.xbase_blanks = true;
        env.engine = true;
        let count = used.count.map(|count| (table.file_changes(), count));
        SqlFilter {
            fields: base.clone(),
            condition,
            env,
            constant,
            superset,
            count,
        }
    });
    // An `ORDER BY` reads every record through the filter to make its index,
    // so a record the filter fails on fails the statement
    // (`fixtures/sql/wherefilter.json`).
    if let (Some(filter), false) = (&sql, plan.order.is_empty()) {
        table
            .scan(|slot, bytes, memo| filter.holds(bytes, slot, memo).map(|_| ()))
            .map_err(|e| match e {
                Error::Evaluation(_) => Error::Sql(tflip_sql::SqlError::filter_division()),
                e => e,
            })?;
    }
    // The records in the order of the `ORDER BY`, which a write puts them in
    // again.
    let sorter = match (plan.order.is_empty(), &used.indexes.order) {
        (true, _) => None,
        (false, Some(name)) if used.sequence.is_some() => {
            let (name, backwards) = (name.clone(), used.backwards);
            Some(table::Sorter::new(move |orders, _| {
                let order = orders.iter().find(|order| order.tag.name == name);
                let mut records =
                    order.map_or(Ok(Vec::new()), |o| o.records_between(None, None))?;
                if backwards {
                    records.reverse();
                }
                Ok(records)
            }))
        }
        (false, _) => {
            let (order, base, env) = (plan.order.clone(), base.clone(), env.clone());
            Some(table::Sorter::new(move |_, scan| {
                let mut keyed: Vec<(u32, Vec<V>)> = Vec::new();
                scan(&mut |slot, bytes, memo| {
                    let mut row = SqlRow {
                        fields: &base,
                        bytes,
                        memo,
                        record: None,
                        hidden: &[],
                    };
                    let mut key = Vec::with_capacity(order.len());
                    for sort in &order {
                        key.push(tflip_sql::eval(&sort.node, &mut row, &env).map_err(Error::Sql)?);
                    }
                    keyed.push((slot, key));
                    Ok(())
                })?;
                // As the original engine's temporary index orders them:
                // walked backwards if every key is descending (the others are
                // written `Descend()`), NULL text first in it whichever way its
                // own key goes, and equal keys in the order of the records.
                let collation = env.collation.as_deref();
                let descending = order.iter().all(|sort| sort.descending);
                keyed.sort_by(|a, b| {
                    a.1.iter()
                        .zip(&b.1)
                        .zip(&order)
                        .map(|((x, y), sort)| {
                            let o = tflip_sql::value::order_indexed(
                                x,
                                y,
                                sort.node.ty.case_insensitive(),
                                collation,
                            );
                            let null_text = sort.node.ty.is_text() && (x.is_null() || y.is_null());
                            let reversed = if null_text {
                                descending
                            } else {
                                sort.descending
                            };
                            if reversed { o.reverse() } else { o }
                        })
                        .find(|o| *o != Ordering::Equal)
                        .unwrap_or_else(|| {
                            if descending {
                                b.0.cmp(&a.0)
                            } else {
                                a.0.cmp(&b.0)
                            }
                        })
                });
                Ok(keyed.into_iter().map(|(slot, _)| slot).collect())
            }))
        }
    };
    let sequence = match (&sorter, used.sequence) {
        (None, _) => None,
        (Some(_), Some(sequence)) => Some(sequence),
        (Some(sorter), None) => Some(sorter.sort(&[], &mut |f| table.scan(f))?),
    };
    // An `ORDER BY` no order serves is read in an index of its own
    // (`fixtures/sql/orderby.json`).
    let temporary = match (&used.indexes.order, plan.order.is_empty()) {
        (None, false) => match tflip_sql::temp_index(&plan.order, &base) {
            tflip_sql::Order::Index(index) => Some(index),
            _ => None,
        },
        _ => None,
    }
    .map(|spec| (format!("TIM_{:X}", next_temporary()), spec));
    let order = used
        .indexes
        .order
        .clone()
        .or_else(|| temporary.as_ref().map(|(name, _)| name.clone()));
    let backwards = used.backwards.then(|| used.indexes.order.clone()).flatten();
    table.make_cursor(
        statement,
        crate::table::LiveCursor {
            fields,
            sql,
            sequence,
            sorter,
            constrain,
            order,
        },
    );
    // The client opens the table of its cursor itself, which a dictionary's
    // `OPEN_TABLE` triggers may cancel.
    let name = table
        .alias
        .get()
        .cloned()
        .unwrap_or_else(|| table::stem(&table.path.to_string_lossy()));
    crate::trigger::fire_open(session, table.connection, &name, &table.settings, true, 1)?;
    let opened =
        crate::trigger::fire_open(session, table.connection, &name, &table.settings, true, 4)?;
    let connection = table.connection;
    let handle = table::register(session, table, Some(statement))?;
    if let Some(opened) = opened {
        crate::trigger::attach(session, connection, handle, opened);
    }
    if let Some(name) = backwards {
        for order in crate::index::open_on(session, handle) {
            if order.tag.name == name {
                order.set_reversed(true);
            }
        }
    }
    if let Some((name, spec)) = temporary {
        crate::index::open_temporary(session, handle, &name, &spec)?;
    }
    Ok(handle)
}

/// Whether `select` has no grouping, aggregates, `DISTINCT`, `UNION` or
/// `INTO`, whatever it reads.
fn plain_select(select: &Select) -> bool {
    use tflip_sql::ast::{ExprKind, Item};
    let aggregate =
        |e: &tflip_sql::ast::Expr| e.any(&|e| matches!(e.kind, ExprKind::Aggregate { .. }));
    select.unions.is_empty()
        && !select.distinct
        && select.group.is_empty()
        && select.having.is_none()
        && select.into.is_none()
        && select.items.iter().all(|item| match item {
            Item::Expr { expr, .. } => !aggregate(expr),
            Item::Star { .. } => true,
        })
}

/// A cursor of a script on a `SELECT` of one table, which reads the records
/// as each `FETCH` comes to them (`fixtures/sql/cursorlive.json`): an update
/// after `OPEN` is seen, a record deleted is skipped, and a `WHERE` holds
/// for the record as it is then. Which records it comes to is fixed earlier:
/// with an `ORDER BY` (or a `TOP` the rows are cut by) it is the list of
/// records `OPEN` found, in order, so an insert is not seen; without, it is the table's records in order, up to the last the
/// table has when the cursor comes to it.
struct OneCursor {
    tables: Opened,
    /// The table's number in the statement.
    id: usize,
    /// The `SELECT` for one record, with no `ORDER BY` or `TOP`.
    one: tflip_sql::query::Prepared,
    /// For a join read by combinations of records: the `SELECT` with no
    /// condition.
    loose: Option<(tflip_sql::query::Prepared, Env)>,
    /// For a join with a `LEFT JOIN` whose `ON` has more than its link: the
    /// join with the `ON`s alone, which a combination of records is read
    /// through to see whether it matches.
    matcher: Option<(tflip_sql::query::Prepared, Env)>,
    right: Option<RightPass>,
    env: Env,
    columns: Vec<Field>,
    types: Vec<Ty>,
    scan: Scan,
}

/// What a `RIGHT JOIN` of two tables has left to read after the inner join
/// of them (`fixtures/sql/cursormore5.json`): the records of the second that
/// no combination had.
struct RightPass {
    first: usize,
    second: usize,
    /// The records of the second table the inner join read.
    hit: std::collections::BTreeSet<u32>,
    /// The next record of the second table to come to.
    next: u32,
}

enum Scan {
    /// The records `OPEN` found, in the order it found them.
    List { records: Vec<u32>, next: usize },
    /// The records in the order of an index, which the cursor walks a leaf
    /// at a time (see [`indexed_order`]).
    Index(IndexWalk),
    /// The records of the first table of a join, each with those of the
    /// second that match it (see [`JoinWalk`]).
    Join(JoinWalk),
    /// Every record of the table, the first `top` rows of them if given.
    Table {
        next: u32,
        top: Option<u64>,
        shown: u64,
    },
}

/// A cursor walking the order of an index on one field, as the original does
/// for an `ORDER BY` of that field (`fixtures/sql/cursormore2.json`): it takes
/// the keys of a leaf of the index when it first needs them (at the first
/// `FETCH`, not at `OPEN`), and reads the records of those as they are by
/// then, whatever changed their keys since; where the leaf ends it finds
/// the last record's place in the index as that record's key is now, and
/// takes the leaf after it. So a change to the index is seen from the next
/// leaf on, and a record whose key moved there is seen again.
struct IndexWalk {
    order: Arc<crate::index::Index>,
    backwards: bool,
    /// The records of the leaf in hand not yet read, with their keys as the
    /// leaf has them.
    leaf: std::collections::VecDeque<tflip_storage::order::Key>,
    /// The last record read, and the key its entry had.
    last: Option<tflip_storage::order::Key>,
    /// Whether the walk has gone past the last leaf.
    ended: bool,
    top: Option<u64>,
    shown: u64,
}

/// The order a cursor on `select` walks if it reads one table's records in
/// the order of the index of its one `ORDER BY` column (the index of a field
/// whose values it holds as bytes); `None` if it does not. The original does
/// not when a column the `WHERE` reads has an index too, where it may build
/// a bitmap of the records that match at `OPEN` (the rule it follows is not
/// known, so these keep their records fixed at `OPEN`).
fn indexed_order(tables: &Opened, id: usize, select: &Select) -> Option<IndexWalk> {
    use tflip_sql::ast::{Expr, ExprKind, Item, OrderKey};
    let read = tables.read(id)?;
    let table = read.table.as_ref()?;
    let [order] = select.order.as_slice() else {
        return None;
    };
    if select
        .top
        .as_ref()
        .is_some_and(|t| t.percent || t.start != 1)
    {
        return None;
    }
    let name = match &order.key {
        OrderKey::Expr(Expr {
            kind: ExprKind::Column { name, .. },
            ..
        }) => name,
        OrderKey::Ordinal(n, _) => {
            let item = select
                .items
                .get(usize::try_from(*n).ok()?.checked_sub(1)?)?;
            match item {
                Item::Expr {
                    expr:
                        Expr {
                            kind: ExprKind::Column { name, .. },
                            ..
                        },
                    ..
                } => name,
                _ => return None,
            }
        }
        OrderKey::Expr(_) => return None,
    };
    let number = |name: &str| {
        table
            .fields()
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))
    };
    let field = number(name)?;
    let lookup = read
        .orders
        .iter()
        .find(|l| l.field == field && l.fields.len() == 1)?;
    let text = table.fields()[field].field_type == tflip_storage::field::FieldType::Character;
    if lookup.order.shared.is_none() || lookup.order.tag.descending || text && lookup.collated {
        return None;
    }
    let indexed: Vec<usize> = crate::index::private_orders(table, true)
        .iter()
        .filter_map(|o| match o.key_source(table) {
            crate::seek::KeySource::Field(f) => {
                table.fields().iter().position(|g| std::ptr::eq(g, f))
            }
            _ => None,
        })
        .collect();
    let mut filtered = false;
    if let Some(filter) = &select.filter {
        filter.walk(&mut |e| {
            if let ExprKind::Column { name, .. } = &e.kind
                && number(name).is_some_and(|f| indexed.contains(&f))
            {
                filtered = true;
            }
        });
    }
    if filtered {
        return None;
    }
    Some(IndexWalk {
        order: Arc::clone(&lookup.order),
        backwards: order.descending,
        leaf: std::collections::VecDeque::new(),
        last: None,
        ended: false,
        top: select.top.as_ref().map(|t| t.count),
        shown: 0,
    })
}

impl IndexWalk {
    /// The next record of the order, taking the next leaf if the one in hand
    /// is read.
    fn next(&mut self, table: &table::Table) -> Result<Option<u32>, Error> {
        if self.top.is_some_and(|top| self.shown >= top) {
            return Ok(None);
        }
        if self.leaf.is_empty() && !self.ended {
            let after = self.last.as_ref().map(|last| {
                // The key the record has now, or its entry's if the record
                // is deleted (the index no longer has its entry, so the
                // walk goes on from where it was).
                let mut active = false;
                let _ = table.read_active(last.record, |_, _| {
                    active = true;
                    Ok(())
                });
                let key = match table.record_key(&self.order, last.record) {
                    Ok(Some(key)) if active => key,
                    _ => last.bytes.clone(),
                };
                (key, last.record)
            });
            let (keys, more) = self.order.walk_leaf(
                self.backwards,
                after
                    .as_ref()
                    .map(|(key, record)| (key.as_slice(), *record)),
            )?;
            self.ended = keys.is_empty() || !more;
            self.leaf.extend(keys);
        }
        let Some(key) = self.leaf.pop_front() else {
            return Ok(None);
        };
        let record = key.record;
        self.last = Some(key);
        Ok(Some(record))
    }
}

/// A cursor on a join of two tables or more, each one after the first linked
/// to an earlier one by the equality of a column of each, and nothing else in
/// the `ON` and `WHERE` (`fixtures/sql/cursormore3.json` and
/// `cursormore4.json`): it goes through the records of the first table as
/// they are when it comes to them (one added meanwhile is read too; the first
/// it has already read at `OPEN`), and for each finds the records of the next
/// table that match its value now: by that table's index on its column, as
/// the table is then (a change to the key is seen), or, with no index, among
/// the records that had that value at `OPEN`; and so on down the tables. Each
/// combination is read as the records are, and kept if the join's condition
/// still holds for them. A `LEFT JOIN`'s table with no match gives one row
/// of nulls.
struct JoinWalk {
    /// The tables in the order of the `FROM`: the first is walked, each of
    /// the others is found through its link.
    steps: Vec<Step>,
    driver: Driver,
    /// The records of the first row at `OPEN` and their values then (the
    /// first table's, and those between it and the last of three or more),
    /// which the cursor reads as they were, whatever happens to the records.
    cached: Vec<(usize, u32, Vec<Value>)>,
    /// The record each table is at in the combination the walk is on (0 for
    /// the nulls of a `LEFT JOIN`).
    chosen: Vec<u32>,
    /// The records of each table left for the combination's earlier ones.
    candidates: Vec<std::collections::VecDeque<u32>>,
    /// Whether a condition reads more than one table beyond the links.
    residual: bool,
    /// Whether a `LEFT JOIN`'s `ON` has more than its link: the records of
    /// the second table that match are those the `ON` holds for.
    extra: bool,
    /// Whether the first row is read again, as the walk stands at it, by the
    /// next `FETCH`.
    replay: bool,
    /// How many tables of `chosen` are set (0: the first's next record is
    /// needed).
    level: usize,
}

struct Step {
    id: usize,
    /// What the select calls the table.
    name: String,
    left: bool,
    /// The column the link is on, and its type.
    field: usize,
    ty: Ty,
    /// The earlier table and column it is equal to (none for the first).
    link: Option<Link>,
    /// The order of the table the lookups go through (a number in its
    /// lookup orders), if its column has one.
    order: Option<usize>,
    /// The table's records by the key they had at `OPEN`, in the order of
    /// the key, if it has no such order: the temporary index the original
    /// builds.
    keys: Vec<(JoinKey, u32)>,
    /// The records of the table a condition on it alone leaves at `OPEN`: the
    /// ones the lookups go through, by their keys as they are now.
    restricted: Option<Vec<u32>>,
}

struct Link {
    step: usize,
    field: usize,
    ty: Ty,
}

/// Where the first table's records come from.
enum Driver {
    /// Every record, as the table has them when the walk comes to them.
    Table { next: u32 },
    /// The records `OPEN` found in the order of an `ORDER BY` of the first
    /// table's columns, an insert after it not among them.
    List { records: Vec<u32>, next: usize },
    /// The combinations of records `OPEN` found in the order of an `ORDER BY`
    /// of other tables' columns (0 for a table of nulls): read as the records
    /// are, with no check of the join's condition.
    Pairs { rows: Vec<Vec<u32>>, next: usize },
}

/// How a join column's value is matched, as the original compares the two
/// columns' values (whole numbers, or text without its trailing blanks).
fn join_key(v: &V) -> Option<JoinKey> {
    match v {
        V::Int(i) => Some(JoinKey::Int(*i)),
        V::Str(s) => Some(JoinKey::Str(s.trim_end().to_owned())),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum JoinKey {
    Int(i64),
    Str(String),
}

/// A column: its table's place in a join's `FROM` and its number there.
type Place = (usize, usize);

/// The tables of `from` if it is a join of tables, an `INNER` or a `LEFT`
/// one at each step, as they come: each with its `ON`.
fn join_leaves<'a>(
    from: &'a tflip_sql::ast::From,
    leaves: &mut Vec<(
        &'a tflip_sql::ast::TableRef,
        bool,
        Option<&'a tflip_sql::ast::Expr>,
    )>,
) -> Option<()> {
    use tflip_sql::ast::{From, JoinKind};
    match from {
        From::Table(table) => leaves.push((table, false, None)),
        From::Derived { .. } => {
            derived_columns(from)?;
            leaves.push((derived_base(from)?, false, None));
        }
        From::Join {
            kind,
            left,
            right,
            on,
        } => {
            join_leaves(left, leaves)?;
            let table = match &**right {
                From::Table(table) => table,
                From::Derived { .. } => {
                    derived_columns(right)?;
                    derived_base(right)?
                }
                _ => return None,
            };
            let outer = match kind {
                JoinKind::Inner => false,
                JoinKind::Left => true,
                _ => return None,
            };
            leaves.push((table, outer, on.as_ref()));
        }
        _ => return None,
    }
    Some(())
}

/// A `SELECT` of columns of one table in `FROM` (`(SELECT k, g FROM u) x`):
/// the name it gives the table and, for each column, its name there and the
/// name it has in the table.
type View = (String, Vec<(String, String)>);

/// The view `from` is, if it is one of columns alone.
fn derived_columns(from: &tflip_sql::ast::From) -> Option<View> {
    use tflip_sql::ast::{ExprKind, From, Item};
    let From::Derived { select, alias, .. } = from else {
        return None;
    };
    if select.filter.is_some() || !matches!(select.from, From::Table(_)) {
        return None;
    }
    let mut columns = Vec::new();
    for item in &select.items {
        let Item::Expr {
            expr:
                tflip_sql::ast::Expr {
                    kind: ExprKind::Column { name, .. },
                    ..
                },
            alias,
        } = item
        else {
            return None;
        };
        columns.push((alias.clone().unwrap_or_else(|| name.clone()), name.clone()));
    }
    Some((alias.clone().unwrap_or_default(), columns))
}

/// The views of the tables of a join's `FROM`, in the order of
/// [`join_leaves`] (none for a table itself).
fn join_views(from: &tflip_sql::ast::From, views: &mut Vec<Option<View>>) {
    use tflip_sql::ast::From;
    match from {
        From::Join { left, right, .. } => {
            join_views(left, views);
            join_views(right, views);
        }
        other => views.push(derived_columns(other)),
    }
}

/// The parts of `condition` joined by `AND`.
fn conjuncts<'a>(condition: &'a tflip_sql::ast::Expr, parts: &mut Vec<&'a tflip_sql::ast::Expr>) {
    use tflip_sql::ast::{BinOp, ExprKind};
    match &condition.kind {
        ExprKind::Binary(BinOp::And, l, r) => {
            conjuncts(l, parts);
            conjuncts(r, parts);
        }
        _ => parts.push(condition),
    }
}

impl JoinWalk {
    /// The walk for `select` if it is a join one reads: its tables, which
    /// the walk starts at, and for each other the one equality that links it
    /// to one before it.
    fn open(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        tables: &Opened,
        select: &Select,
        right: bool,
    ) -> Result<Option<JoinWalk>, Error> {
        use tflip_sql::ast::{BinOp, Expr, ExprKind, From};
        use tflip_sql::query::Tables as _;
        use tflip_storage::field::FieldType::*;
        let mut leaves = Vec::new();
        if join_leaves(&select.from, &mut leaves).is_none()
            || leaves.len() < 2
            || !plain_select(select)
            || select.top.is_some()
        {
            return Ok(None);
        }
        let count = leaves.len();
        let any_left = leaves.iter().any(|l| l.1);
        let mut views = Vec::new();
        join_views(&select.from, &mut views);
        // A view's columns are sorted, joined and found by their names in
        // its `SELECT`, so far for a join that reads the records as they
        // come.
        if views.iter().any(Option::is_some) && !select.order.is_empty() {
            return Ok(None);
        }
        let mut found: Vec<(Option<usize>, &Expr)> = Vec::new();
        let mut parts = Vec::new();
        for (i, (_, _, on)) in leaves.iter().enumerate() {
            if let Some(on) = on {
                conjuncts(on, &mut parts);
                found.extend(parts.drain(..).map(|p| (Some(i), p)));
            }
        }
        if let Some(filter) = &select.filter {
            conjuncts(filter, &mut parts);
            found.extend(parts.drain(..).map(|p| (None, p)));
        }
        let mut open = Vec::new();
        for (table, _, _) in &leaves {
            match tables.table(table.id) {
                Some(t) if !t.is_built() => open.push(t),
                _ => return Ok(None),
            }
        }
        let family = |t: &table::Table, f: usize| match t.fields()[f].field_type {
            ShortInt | Integer | LongInt | AutoInc => Some(0),
            Character => Some(1),
            _ => None,
        };
        // The table a column is of, and its number there.
        let place = |qualifier: &Option<String>, name: &str| -> Option<Place> {
            let mut at = None;
            for (i, (t, _, _)) in leaves.iter().enumerate() {
                let own = match &views[i] {
                    Some((alias, _)) => alias,
                    None => t.alias.as_deref().unwrap_or(&t.name),
                };
                if qualifier
                    .as_deref()
                    .is_some_and(|q| !q.eq_ignore_ascii_case(own))
                {
                    continue;
                }
                let name = match &views[i] {
                    Some((_, columns)) => {
                        match columns.iter().find(|c| c.0.eq_ignore_ascii_case(name)) {
                            Some(column) => &column.1,
                            None => continue,
                        }
                    }
                    None => name,
                };
                if let Some(f) = open[i]
                    .fields()
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(name))
                {
                    if at.is_some() {
                        return None;
                    }
                    at = Some((i, f));
                }
            }
            at
        };
        // What each condition is: the equality of a column of two tables
        // (which may link them), a condition on the columns of one table
        // (which restricts it), or another (which the join checks as it is).
        let mut equalities: Vec<(Place, Place)> = Vec::new();
        let mut restrictions: Vec<Vec<&Expr>> = vec![Vec::new(); count];
        let mut residual = false;
        // Whether a condition joins two tables.
        let mut spans = false;
        // A `LEFT JOIN`'s `ON` has more than its link.
        let mut extra = false;
        for (tag, condition) in found {
            let eq = match &condition.kind {
                ExprKind::Binary(BinOp::Eq, l, r) => match (&l.kind, &r.kind) {
                    (
                        ExprKind::Column {
                            table: lt,
                            name: ln,
                        },
                        ExprKind::Column {
                            table: rt,
                            name: rn,
                        },
                    ) => place(lt, ln).zip(place(rt, rn)),
                    _ => None,
                },
                _ => None,
            };
            if let Some((x, y)) = eq
                && x.0 != y.0
                && family(open[x.0], x.1).is_some()
                && family(open[x.0], x.1) == family(open[y.0], y.1)
            {
                // A `LEFT JOIN`'s own `ON` decides what it matches, and the
                // `ON` of a join is about its own table.
                let later = x.0.max(y.0);
                if tag.is_some_and(|t| t != later) || (tag.is_none() && leaves[later].1) {
                    return Ok(None);
                }
                equalities.push((x, y));
                spans = true;
                continue;
            }
            let mut read = std::collections::BTreeSet::new();
            let mut known = true;
            condition.walk(&mut |e| match &e.kind {
                ExprKind::Column { table, name } => match place(table, name) {
                    Some((i, _)) => {
                        read.insert(i);
                    }
                    None => known = false,
                },
                ExprKind::Subquery(_)
                | ExprKind::Exists(_)
                | ExprKind::InSelect { .. }
                | ExprKind::Quantified { .. } => known = false,
                _ => {}
            });
            // What is left of a `LEFT JOIN`'s `ON` would give its row of
            // nulls, not drop the row.
            if !known {
                return Ok(None);
            }
            if tag.is_some_and(|i| leaves[i].1) {
                // The `ON` of the one `LEFT JOIN` of two tables decides
                // which records of the second match each of the first.
                if count != 2 {
                    return Ok(None);
                }
                extra = true;
                continue;
            }
            match (read.len(), read.first()) {
                (1, Some(&i)) if !leaves[i].1 => restrictions[i].push(condition),
                (1, _) => {}
                _ => {
                    residual = true;
                    spans |= read.len() > 1;
                }
            }
        }
        // A `LEFT JOIN` with more in its `ON` than the link goes through the
        // records of its table as they are, whatever they have as the key.
        if extra {
            equalities.clear();
        }
        // The records of each table the restriction on it leaves at `OPEN`
        // (all of them for the others, counted if a table is to be chosen).
        let choose = !any_left && select.order.is_empty() && !right;
        let mut sets: Vec<Option<Vec<u32>>> = vec![None; count];
        let mut sizes = vec![0usize; count];
        for i in 0..count {
            if views[i].is_some() && !restrictions[i].is_empty() {
                return Ok(None);
            }
            if restrictions[i].is_empty() && !choose {
                continue;
            }
            let mut listed = select.clone();
            listed.from = From::Table(leaves[i].0.clone());
            listed.items.clear();
            listed.order.clear();
            listed.filter = restrictions[i]
                .iter()
                .map(|&e| e.clone())
                .reduce(|l, r| Expr {
                    kind: ExprKind::Binary(BinOp::And, Box::new(l), Box::new(r)),
                    pos: 0,
                });
            let records = listed_records(session, connection, settings, tables, listed, &[])?;
            sizes[i] = records.len();
            if !restrictions[i].is_empty() {
                sets[i] = Some(records.into_iter().map(|r| r[0]).collect());
            }
        }
        // Tables no condition joins are looped with the last as the outer
        // one, if no condition leaves one of them fewer records.
        let cross = count == 2 && !spans && !any_left;
        let first = match choose {
            true => (0..count)
                .min_by_key(|&i| (sizes[i], if cross { count - i } else { i }))
                .unwrap_or(0),
            false => 0,
        };
        // The tables in the order the walk goes through them: from the
        // first, each next one the first in the `FROM` with an equality to
        // one already in.
        let mut order = vec![first];
        let mut links: Vec<Option<(Place, Place)>> = vec![None; count];
        while order.len() < count {
            let next = (0..count).filter(|i| !order.contains(i)).find_map(|i| {
                equalities.iter().find_map(|&(x, y)| match (x, y) {
                    (a, b) if b.0 == i && order.contains(&a.0) => Some((i, a, b)),
                    (b, a) if b.0 == i && order.contains(&a.0) => Some((i, a, b)),
                    _ => None,
                })
            });
            let Some((i, from, to)) = next else {
                // A table no equality links is read whole for each
                // combination of the others.
                if equalities.is_empty() && count > 2 {
                    return Ok(None);
                }
                order.extend((0..count).find(|i| !order.contains(i)));
                continue;
            };
            order.push(i);
            links[i] = Some((from, to));
        }
        let mut steps = Vec::new();
        for &i in &order {
            let (table, left, _) = leaves[i];
            let fields = open[i].fields();
            let (link, field) = match links[i] {
                None => (None, 0),
                Some((from, to)) => (
                    Some(Link {
                        step: order.iter().position(|&j| j == from.0).unwrap_or(0),
                        field: from.1,
                        ty: Ty::of_field(&open[from.0].fields()[from.1]),
                    }),
                    to.1,
                ),
            };
            let order = (link.is_some() && sets[i].is_none())
                .then(|| {
                    tables.read(table.id).and_then(|read| {
                        read.orders
                            .iter()
                            .position(|l| l.field == field && l.fields.len() == 1 && !l.collated)
                    })
                })
                .flatten();
            steps.push(Step {
                id: table.id,
                name: table.alias.clone().unwrap_or_else(|| table.name.clone()),
                left,
                field,
                ty: Ty::of_field(&fields[field]),
                link,
                order,
                keys: Vec::new(),
                restricted: if i == first { None } else { sets[i].take() },
            });
        }
        let driver = match sets[first].take() {
            Some(records) => Driver::List { records, next: 0 },
            None => Driver::Table { next: 0 },
        };
        let mut walk = JoinWalk {
            chosen: vec![0; count],
            candidates: vec![std::collections::VecDeque::new(); count],
            steps,
            driver,
            cached: Vec::new(),
            residual: residual && !(equalities.is_empty()),
            extra,
            replay: false,
            level: 0,
        };
        for i in 1..count {
            if walk.steps[i].order.is_some()
                || walk.steps[i].restricted.is_some()
                || walk.steps[i].link.is_none()
            {
                continue;
            }
            let (id, field, ty) = (walk.steps[i].id, walk.steps[i].field, walk.steps[i].ty);
            let table = tables.table(id).map_or(0, table::Table::slot_count);
            let mut keys = Vec::new();
            for slot in 1..=table {
                tables
                    .fetch(id, &[slot], &mut |record, row| {
                        if let Some(key) = join_key(&row.column(field, ty)?) {
                            keys.push((key, record));
                        }
                        Ok(())
                    })
                    .map_err(|e| tables.error(e))?;
            }
            keys.sort();
            walk.steps[i].keys = keys;
        }
        Ok(Some(walk))
    }

    /// Keeps the records of the first row of the join at `OPEN` as they are
    /// now, which the cursor reads as they are from here on: the first
    /// table's, and with three tables or more those between it and the last.
    fn cache(&mut self, tables: &Opened) -> Result<(), Error> {
        use tflip_sql::query::Tables as _;
        let kept = if self.steps.len() > 2 {
            self.steps.len() - 1
        } else {
            1
        };
        self.cached.clear();
        for (step, &record) in self.steps.iter().zip(&self.chosen).take(kept) {
            let count = tables.table(step.id).map_or(0, |t| t.fields().len());
            let mut values = Vec::new();
            tables
                .fetch(step.id, &[record], &mut |_, row| {
                    for i in 0..count {
                        values.push(row.value(i)?);
                    }
                    Ok(())
                })
                .map_err(|e| tables.error(e))?;
            if !values.is_empty() {
                self.cached.push((step.id, record, values));
            }
        }
        Ok(())
    }

    /// Goes back to before the first record.
    fn rewind(&mut self) {
        match &mut self.driver {
            Driver::Table { next } => *next = 0,
            Driver::List { next, .. } | Driver::Pairs { next, .. } => *next = 0,
        }
        self.level = 0;
    }

    /// The next record of the first table the walk comes to.
    fn next_driver(&mut self, tables: &Opened) -> Result<Option<u32>, Error> {
        use tflip_sql::query::Tables as _;
        let id = self.steps[0].id;
        loop {
            let record = match &mut self.driver {
                Driver::Table { next } => {
                    let last = tables.table(id).map_or(0, table::Table::slot_count);
                    if *next >= last {
                        return Ok(None);
                    }
                    *next += 1;
                    *next
                }
                Driver::List { records, next } => {
                    let Some(&record) = records.get(*next) else {
                        return Ok(None);
                    };
                    *next += 1;
                    record
                }
                Driver::Pairs { .. } => return Ok(None),
            };
            // A record deleted since is passed over.
            let mut there = self.cached.iter().any(|(i, r, _)| *i == id && *r == record);
            if !there {
                tables
                    .fetch(id, &[record], &mut |_, _| {
                        there = true;
                        Ok(())
                    })
                    .map_err(|e| tables.error(e))?;
            }
            if there {
                return Ok(Some(record));
            }
        }
    }

    /// The records of the table at `level` that match the one `chosen`
    /// holds for the table it is linked to.
    fn prepare(&mut self, tables: &Opened, level: usize) -> Result<(), Error> {
        use tflip_sql::Row as _;
        use tflip_sql::query::Tables as _;
        let Some(link) = &self.steps[level].link else {
            // No link: the records the table has, or the restriction left.
            let step = &self.steps[level];
            let found: Vec<u32> = match &step.restricted {
                Some(records) => records.clone(),
                None => (1..=tables.table(step.id).map_or(0, table::Table::slot_count)).collect(),
            };
            self.candidates[level] = match found.is_empty() && step.left {
                true => vec![0],
                false => found,
            }
            .into();
            return Ok(());
        };
        let from = &self.steps[link.step];
        let record = self.chosen[link.step];
        let mut value = None;
        let cached = self
            .cached
            .iter()
            .find(|(id, number, _)| *id == from.id && *number == record);
        match cached {
            _ if record == 0 => {}
            Some((_, _, values)) => {
                value = Some(
                    Values(values)
                        .column(link.field, link.ty)
                        .map_err(|e| tables.error(e))?,
                );
            }
            None => tables
                .fetch(from.id, &[record], &mut |_, row| {
                    value = Some(row.column(link.field, link.ty)?);
                    Ok(())
                })
                .map_err(|e| tables.error(e))?,
        }
        let step = &self.steps[level];
        let found = match value.filter(|v| !matches!(v, V::Null)) {
            None => Vec::new(),
            Some(value) if step.restricted.is_some() => {
                let mut found = Vec::new();
                let key = join_key(&value);
                for &record in step.restricted.iter().flatten() {
                    let mut now = None;
                    tables
                        .fetch(step.id, &[record], &mut |_, row| {
                            now = Some(join_key(&row.column(step.field, step.ty)?));
                            Ok(())
                        })
                        .map_err(|e| tables.error(e))?;
                    if now.is_some_and(|now| now == key) {
                        found.push(record);
                    }
                }
                found
            }
            Some(value) => match step.order {
                Some(order) => tables
                    .lookup(step.id, order, Some(&value), Some(&value))
                    .map_err(|e| tables.error(e))?,
                None => {
                    // From the first entry of the key (as it was) on, those
                    // whose records have the key now, up to one that has not.
                    let mut found = Vec::new();
                    if let Some(key) = join_key(&value) {
                        let start = step.keys.partition_point(|(k, _)| *k < key);
                        for &(_, record) in &step.keys[start..] {
                            let mut now = None;
                            tables
                                .fetch(step.id, &[record], &mut |_, row| {
                                    now = Some(join_key(&row.column(step.field, step.ty)?));
                                    Ok(())
                                })
                                .map_err(|e| tables.error(e))?;
                            match now {
                                // A deleted record is passed over.
                                None => continue,
                                Some(now) if now.as_ref() == Some(&key) => found.push(record),
                                Some(_) => break,
                            }
                        }
                    }
                    found
                }
            },
        };
        self.candidates[level] = match found.is_empty() && step.left {
            true => vec![0],
            false => found,
        }
        .into();
        Ok(())
    }

    /// The table of each step with the record the walk has for it.
    fn fixed(&self) -> Vec<(usize, u32)> {
        self.steps
            .iter()
            .zip(&self.chosen)
            .map(|(step, &record)| (step.id, record))
            .collect()
    }
}

/// Whether the `ORDER BY` of `select` is of columns of its first table
/// alone (named as they are, not by the select list's aliases or places).
fn orders_by_first(
    tables: &Opened,
    select: &Select,
    leaves: &[(
        &tflip_sql::ast::TableRef,
        bool,
        Option<&tflip_sql::ast::Expr>,
    )],
) -> bool {
    use tflip_sql::ast::{Expr, ExprKind, Item, OrderKey};
    let has = |id: usize, name: &str| {
        tables
            .table(id)
            .is_some_and(|t| t.fields().iter().any(|f| f.name.eq_ignore_ascii_case(name)))
    };
    let first = leaves[0].0;
    select.order.iter().all(|item| {
        let OrderKey::Expr(Expr {
            kind: ExprKind::Column { table, name },
            ..
        }) = &item.key
        else {
            return false;
        };
        match table {
            Some(q) => {
                q.eq_ignore_ascii_case(first.alias.as_deref().unwrap_or(&first.name))
                    && has(first.id, name)
            }
            None => {
                has(first.id, name)
                    && !leaves[1..].iter().any(|(t, _, _)| has(t.id, name))
                    && !select.items.iter().any(|i| {
                        matches!(i, Item::Expr { alias: Some(a), .. } if a.eq_ignore_ascii_case(name))
                    })
            }
        }
    })
}

/// `from` with each join made `INNER`, its `ON` kept.
fn inner_joins(from: &mut tflip_sql::ast::From) {
    if let tflip_sql::ast::From::Join { kind, left, .. } = from {
        *kind = tflip_sql::ast::JoinKind::Inner;
        inner_joins(left);
    }
}

/// `from` with each join's `ON` taken off and made `INNER`.
fn clear_joins(from: &mut tflip_sql::ast::From) {
    if let tflip_sql::ast::From::Join { kind, left, on, .. } = from {
        *kind = tflip_sql::ast::JoinKind::Inner;
        *on = None;
        clear_joins(left);
    }
}

/// The record numbers of the tables `names` (one record's, if none) for each
/// row `listed` gives, in the order the rows have.
fn listed_records(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    tables: &Opened,
    mut listed: Select,
    names: &[&str],
) -> Result<Vec<Vec<u32>>, Error> {
    use tflip_sql::ast::{Expr, ExprKind, Item};
    let call = |args: Vec<Expr>| Item::Expr {
        expr: Expr {
            kind: ExprKind::Func {
                name: "RECNO".to_owned(),
                args,
            },
            pos: 0,
        },
        alias: None,
    };
    if names.is_empty() {
        listed.items.push(call(Vec::new()));
    }
    for name in names {
        listed.items.push(call(vec![Expr {
            kind: ExprKind::Column {
                table: None,
                name: (*name).to_owned(),
            },
            pos: 0,
        }]));
    }
    let count = names.len().max(1);
    let prepared = tflip_sql::query::prepare(&listed, tables, 0).map_err(|e| tables.error(e))?;
    let env = env(session, connection, Some(settings), &[], &prepared.params)?;
    let output = prepared.run(tables, &env).map_err(|e| tables.error(e))?;
    let mut rows = output.rows;
    if let Some(sequence) = &output.sequence {
        rows = sequence
            .iter()
            .map(|&n| std::mem::take(&mut rows[n as usize - 1]))
            .collect();
    }
    Ok(rows
        .iter()
        .map(|row| {
            row[row.len() - count..]
                .iter()
                .map(|v| match v {
                    V::Int(n) => u32::try_from(*n).unwrap_or(0),
                    _ => 0,
                })
                .collect()
        })
        .collect())
}

/// The tables of a [`LiveCursor`] for the one record of each it reads (the
/// table of a `SELECT` of one, or the two of a join), which is all that those
/// tables have. A record of them may be read as it was (`cached`: its table,
/// its number and its values).
struct OneRecord<'a> {
    opened: &'a Opened,
    fixed: &'a [(usize, u32)],
    cached: &'a [(usize, u32, Vec<Value>)],
}

/// A record's values as they were.
struct Values<'a>(&'a [Value]);

impl tflip_sql::Row for Values<'_> {
    fn value(&mut self, index: usize) -> Result<Value, SqlError> {
        Ok(self.0.get(index).cloned().unwrap_or(Value::Null))
    }
}

impl OneRecord<'_> {
    fn record(&self, id: usize) -> Option<u32> {
        self.fixed.iter().find(|f| f.0 == id).map(|f| f.1)
    }
}

impl tflip_sql::query::Tables for OneRecord<'_> {
    fn fields(&self, id: usize) -> &[Field] {
        self.opened.fields(id)
    }

    fn functions(&self) -> Vec<tflip_sql::FunctionSig> {
        self.opened.functions()
    }

    fn user(&self) -> Option<String> {
        self.opened.user()
    }

    fn info(&self, id: usize) -> tflip_sql::compile::TableInfo {
        self.opened.info(id)
    }

    fn scan(
        &self,
        id: usize,
        f: &mut dyn FnMut(&mut dyn tflip_sql::Row) -> Result<(), SqlError>,
    ) -> Result<(), SqlError> {
        let Some(record) = self.record(id) else {
            return self.opened.scan(id, f);
        };
        // The nulls of a `LEFT JOIN`: no record.
        if record == 0 {
            return Ok(());
        }
        if let Some((_, _, values)) = self
            .cached
            .iter()
            .find(|(cached, number, _)| *cached == id && *number == record)
        {
            return f(&mut Values(values));
        }
        self.opened.fetch(id, &[record], &mut |_, row| f(row))
    }

    fn indexes(&self, id: usize) -> Vec<tflip_sql::query::TableIndex> {
        match self.record(id) {
            Some(_) => Vec::new(),
            None => self.opened.indexes(id),
        }
    }

    fn fts(&self, id: usize) -> Vec<Option<tflip_sql::fts::Rules>> {
        self.opened.fts(id)
    }

    fn fts_lookup(
        &self,
        id: usize,
        column: Option<usize>,
        query: &tflip_sql::fts::Query,
    ) -> Result<Option<tflip_sql::query::FtsLookup>, SqlError> {
        match self.record(id) {
            Some(_) => Ok(None),
            None => self.opened.fts_lookup(id, column, query),
        }
    }

    fn lookup(
        &self,
        id: usize,
        index: usize,
        low: Option<&V>,
        high: Option<&V>,
    ) -> Result<Vec<u32>, SqlError> {
        self.opened.lookup(id, index, low, high)
    }

    fn fetch(
        &self,
        id: usize,
        records: &[u32],
        f: &mut dyn FnMut(u32, &mut dyn tflip_sql::Row) -> Result<(), SqlError>,
    ) -> Result<(), SqlError> {
        self.opened.fetch(id, records, f)
    }
}

impl OneCursor {
    /// Opens a cursor on `select` that reads its table live, if it is one
    /// that does.
    fn open(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        select: &Select,
    ) -> Result<Option<OneCursor>, Error> {
        use tflip_sql::ast::{Expr, ExprKind, From, Item};
        if matches!(select.from, From::Join { .. }) {
            return Self::open_join(session, connection, settings, select);
        }
        let Some(table_ref) = (match &select.from {
            From::Table(table) => Some(table),
            From::Derived { .. } => derived_base(&select.from),
            _ => None,
        }) else {
            return Ok(None);
        };
        if !plain_select(select) {
            return Ok(None);
        }
        let id = table_ref.id;
        let tables = Opened::open(
            session,
            connection,
            settings,
            select,
            Procedures::Run {
                statement: None,
                params: &[],
            },
        )?;
        let Some(table) = tables.table(id) else {
            return Ok(None);
        };
        if table.is_built() {
            return Ok(None);
        }
        let ordered = !select.order.is_empty()
            || select
                .top
                .as_ref()
                .is_some_and(|t| t.percent || t.start != 1);
        let mut one = select.clone();
        one.order.clear();
        one.top = None;
        let at = OneRecord {
            opened: &tables,
            fixed: &[(id, 0)],
            cached: &[],
        };
        let one = tflip_sql::query::prepare(&one, &at, 0).map_err(|e| tables.error(e))?;
        let env_one = env(session, connection, Some(settings), &[], &one.params)?;
        let columns: Vec<Field> = columns_of(&one.columns).iter().map(field_of).collect();
        let types = columns.iter().map(Ty::of_field).collect();
        let indexed = match ordered {
            true => indexed_order(&tables, id, select),
            false => None,
        };
        let scan = if let Some(walk) = indexed {
            Scan::Index(walk)
        } else if ordered {
            // The records in the order the rows have, found with their
            // numbers.
            let mut listed = select.clone();
            listed.items.push(Item::Expr {
                expr: Expr {
                    kind: ExprKind::Func {
                        name: "RECNO".to_owned(),
                        args: Vec::new(),
                    },
                    pos: 0,
                },
                alias: None,
            });
            let prepared =
                tflip_sql::query::prepare(&listed, &tables, 0).map_err(|e| tables.error(e))?;
            let env = env(session, connection, Some(settings), &[], &prepared.params)?;
            let output = prepared.run(&tables, &env).map_err(|e| tables.error(e))?;
            let mut rows = output.rows;
            if let Some(sequence) = &output.sequence {
                rows = sequence
                    .iter()
                    .map(|&n| std::mem::take(&mut rows[n as usize - 1]))
                    .collect();
            }
            let records = rows
                .iter()
                .filter_map(|row| match row.last() {
                    Some(V::Int(n)) => u32::try_from(*n).ok(),
                    _ => None,
                })
                .collect();
            Scan::List { records, next: 0 }
        } else {
            Scan::Table {
                next: 0,
                top: select.top.as_ref().map(|t| t.count),
                shown: 0,
            }
        };
        Ok(Some(OneCursor {
            tables,
            id,
            one,
            loose: None,
            matcher: None,
            right: None,
            env: env_one,
            columns,
            types,
            scan,
        }))
    }

    /// Opens a cursor on `select` if it is a join that reads live (see
    /// [`JoinWalk`]).
    fn open_join(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        select: &Select,
    ) -> Result<Option<OneCursor>, Error> {
        let tables = Opened::open(
            session,
            connection,
            settings,
            select,
            Procedures::Run {
                statement: None,
                params: &[],
            },
        )?;
        // A `RIGHT JOIN` is the inner join, then the records of the second
        // table no combination had, with nulls for the first.
        let mut inner = None;
        let mut right = None;
        if let tflip_sql::ast::From::Join {
            kind: tflip_sql::ast::JoinKind::Right,
            left,
            right: second,
            on: Some(_),
        } = &select.from
            && let (tflip_sql::ast::From::Table(first), tflip_sql::ast::From::Table(second)) =
                (&**left, &**second)
            && select.order.is_empty()
            && select.filter.is_none()
        {
            let mut select = select.clone();
            if let tflip_sql::ast::From::Join { kind, .. } = &mut select.from {
                *kind = tflip_sql::ast::JoinKind::Inner;
            }
            inner = Some(select);
            right = Some(RightPass {
                first: first.id,
                second: second.id,
                hit: Default::default(),
                next: 1,
            });
        }
        let walked = inner.as_ref().unwrap_or(select);
        let Some(mut walk) = JoinWalk::open(
            session,
            connection,
            settings,
            &tables,
            walked,
            right.is_some(),
        )?
        else {
            return Ok(None);
        };
        let mut loose = None;
        if !select.order.is_empty() {
            // An `ORDER BY` of the first table's columns sorts its records,
            // which the join then goes through; any other sorts the
            // combinations of records.
            let mut leaves = Vec::new();
            join_leaves(&select.from, &mut leaves);
            let first = leaves[0].0;
            let mut listed = select.clone();
            listed.top = None;
            if orders_by_first(&tables, select, &leaves) {
                listed.from = tflip_sql::ast::From::Table(first.clone());
                listed.filter = None;
                listed.items.clear();
                let records = listed_records(session, connection, settings, &tables, listed, &[])?;
                walk.driver = Driver::List {
                    records: records.into_iter().map(|r| r[0]).collect(),
                    next: 0,
                };
            } else {
                let names: Vec<&str> = walk.steps.iter().map(|s| s.name.as_str()).collect();
                let rows = listed_records(session, connection, settings, &tables, listed, &names)?;
                walk.driver = Driver::Pairs { rows, next: 0 };
                // What each combination is read through: the select with no
                // condition.
                let mut select = select.clone();
                select.filter = None;
                select.order.clear();
                clear_joins(&mut select.from);
                loose = Some(select);
            }
        }
        let fixed: Vec<(usize, u32)> = walk.steps.iter().map(|s| (s.id, 0)).collect();
        let prepare = |select: &Select| -> Result<_, Error> {
            let at = OneRecord {
                opened: &tables,
                fixed: &fixed,
                cached: &[],
            };
            let one = tflip_sql::query::prepare(select, &at, 0).map_err(|e| tables.error(e))?;
            let env_one = env(session, connection, Some(settings), &[], &one.params)?;
            Ok((one, env_one))
        };
        let mut first = walked.clone();
        first.order.clear();
        let (one, env_one) = prepare(&first)?;
        if right.is_some() {
            loose = Some(select.clone());
        }
        let loose = loose.map(|s| prepare(&s)).transpose()?;
        // What the records of the second table of a `LEFT JOIN` must hold
        // for: its `ON`.
        let matcher = if walk
            .steps
            .iter()
            .any(|s| s.left && (walk.extra || s.link.is_none()))
        {
            let mut select = walked.clone();
            select.filter = None;
            select.order.clear();
            inner_joins(&mut select.from);
            Some(prepare(&select)?)
        } else {
            None
        };
        let shape = loose
            .as_ref()
            .filter(|_| right.is_some())
            .map_or(&one, |l| &l.0);
        let columns: Vec<Field> = columns_of(&shape.columns).iter().map(field_of).collect();
        let types = columns.iter().map(Ty::of_field).collect();
        let mut cursor = OneCursor {
            id: walk.steps[0].id,
            tables,
            one,
            loose,
            matcher,
            right,
            env: env_one,
            columns,
            types,
            scan: Scan::Join(walk),
        };
        // `OPEN` reads the first row, whose first record is then kept as it
        // is (and with three tables or more, the others of its path but the
        // last, and the records of the path left to find, which the first
        // `FETCH` goes on from).
        if cursor.fetch_join()?.is_some()
            && let Scan::Join(walk) = &mut cursor.scan
            && !matches!(walk.driver, Driver::Pairs { .. })
            && !walk.residual
        {
            walk.cache(&cursor.tables)?;
            walk.replay = walk.steps.len() > 2;
        }
        if let Scan::Join(walk) = &mut cursor.scan
            && !walk.replay
        {
            walk.rewind();
        }
        if let Some(right) = &mut cursor.right {
            right.hit.clear();
            right.next = 1;
        }
        Ok(Some(cursor))
    }

    /// The records of the table at `level` that match the ones before it in
    /// the walk: those its link finds, and for a `LEFT JOIN` that its `ON`
    /// holds for (or the nulls, if there are none).
    fn prepare_level(&mut self, level: usize) -> Result<(), Error> {
        let Scan::Join(walk) = &mut self.scan else {
            return Ok(());
        };
        walk.prepare(&self.tables, level)?;
        let Some((matcher, env)) = &self.matcher else {
            return Ok(());
        };
        if !walk.steps[level].left {
            return Ok(());
        }
        let mut kept = Vec::new();
        for &record in walk.candidates[level].iter().filter(|&&r| r != 0) {
            let mut fixed = walk.fixed();
            fixed[level].1 = record;
            let at = OneRecord {
                opened: &self.tables,
                fixed: &fixed,
                cached: &walk.cached,
            };
            let output = matcher.run(&at, env).map_err(|e| self.tables.error(e))?;
            if !output.rows.is_empty() {
                kept.push(record);
            }
        }
        if kept.is_empty() {
            kept.push(0);
        }
        walk.candidates[level] = kept.into();
        Ok(())
    }

    /// The records of the second table of a `RIGHT JOIN` that no combination
    /// had, each with nulls for the first.
    fn right_rest(&mut self) -> Result<Option<Vec<V>>, Error> {
        loop {
            let Some(right) = &mut self.right else {
                return Ok(None);
            };
            let last = self
                .tables
                .table(right.second)
                .map_or(0, table::Table::slot_count);
            if right.next > last {
                return Ok(None);
            }
            let record = right.next;
            right.next += 1;
            if right.hit.contains(&record) {
                continue;
            }
            let fixed = [(right.first, 0), (right.second, record)];
            if let Some(row) = self.row_of(&fixed, true)? {
                return Ok(Some(row));
            }
        }
    }

    /// The next row of a join: of the next combination of records that
    /// matches.
    fn fetch_join(&mut self) -> Result<Option<Vec<V>>, Error> {
        loop {
            let Scan::Join(walk) = &mut self.scan else {
                return Ok(None);
            };
            if let Driver::Pairs { rows, next } = &mut walk.driver {
                let Some(pair) = rows.get(*next) else {
                    return Ok(None);
                };
                let fixed: Vec<(usize, u32)> = walk
                    .steps
                    .iter()
                    .zip(pair)
                    .map(|(step, &record)| (step.id, record))
                    .collect();
                let nulls = pair.contains(&0);
                *next += 1;
                if let Some(row) = self.row_of(&fixed, !nulls)? {
                    return Ok(Some(row));
                }
                continue;
            }
            if walk.replay {
                walk.replay = false;
                let fixed = walk.fixed();
                if let Some(row) = self.row_of(&fixed, false)? {
                    return Ok(Some(row));
                }
                continue;
            }
            if walk.level == 0 {
                let Some(record) = walk.next_driver(&self.tables)? else {
                    return self.right_rest();
                };
                walk.chosen[0] = record;
                walk.level = 1;
                self.prepare_level(1)?;
                continue;
            }
            let level = walk.level;
            let Some(record) = walk.candidates[level].pop_front() else {
                walk.level -= 1;
                continue;
            };
            walk.chosen[level] = record;
            if level + 1 < walk.steps.len() {
                walk.level = level + 1;
                self.prepare_level(level + 1)?;
                continue;
            }
            let fixed = walk.fixed();
            if let Some(row) = self.row_of(&fixed, false)? {
                if let Some(right) = &mut self.right {
                    right.hit.insert(fixed[1].1);
                }
                return Ok(Some(row));
            }
        }
    }

    /// The cursor's columns.
    pub(crate) fn columns(&self) -> Vec<Field> {
        self.columns.clone()
    }

    /// The next record's row, as the record is now.
    pub(crate) fn fetch(&mut self) -> Result<Option<Vec<V>>, Error> {
        if matches!(self.scan, Scan::Join(_)) {
            return self.fetch_join();
        }
        loop {
            let record = match &mut self.scan {
                Scan::List { records, next } => {
                    let Some(&record) = records.get(*next) else {
                        return Ok(None);
                    };
                    *next += 1;
                    record
                }
                Scan::Join(_) => return Ok(None),
                Scan::Index(walk) => {
                    let Some(table) = self.tables.table(self.id) else {
                        return Ok(None);
                    };
                    match walk.next(table)? {
                        Some(record) => record,
                        None => return Ok(None),
                    }
                }
                Scan::Table { next, top, shown } => {
                    let last = self
                        .tables
                        .table(self.id)
                        .map_or(0, table::Table::slot_count);
                    if *next >= last || top.is_some_and(|top| *shown >= top) {
                        return Ok(None);
                    }
                    *next += 1;
                    *next
                }
            };
            if let Some(row) = self.row_at(record)? {
                match &mut self.scan {
                    Scan::Table { shown, .. } => *shown += 1,
                    Scan::Index(walk) => walk.shown += 1,
                    Scan::List { .. } | Scan::Join(_) => {}
                }
                return Ok(Some(row));
            }
        }
    }

    /// The row of `record` if it is there and the `WHERE` holds for it.
    fn row_at(&self, record: u32) -> Result<Option<Vec<V>>, Error> {
        self.row_of(&[(self.id, record)], false)
    }

    /// The row of the `fixed` records, one of each table, if the `WHERE`
    /// (and `ON`) holds for them; with `loose`, as the records are whatever
    /// they hold.
    fn row_of(&self, fixed: &[(usize, u32)], loose: bool) -> Result<Option<Vec<V>>, Error> {
        let cached = match &self.scan {
            Scan::Join(walk) => walk.cached.as_slice(),
            _ => &[],
        };
        let (one, env) = match (&self.loose, loose) {
            (Some((one, env)), true) => (one, env),
            _ => (&self.one, &self.env),
        };
        let at = OneRecord {
            opened: &self.tables,
            fixed,
            cached,
        };
        let output = one.run(&at, env).map_err(|e| self.tables.error(e))?;
        let Some(row) = output.rows.first() else {
            return Ok(None);
        };
        let mut stored = stored_rows(std::slice::from_ref(row), &one.columns)?;
        if self.tables.oem() {
            for value in stored.iter_mut().flatten() {
                if let Value::Text(text) = value {
                    *text = tflip_sql::compile::oem_to_ansi_text(text);
                }
            }
        }
        Ok(stored.pop().map(|row| {
            row.into_iter()
                .zip(&self.types)
                .map(|(value, ty)| V::from_field(value, *ty))
                .collect()
        }))
    }
}

/// A cursor of a script that reads its tables live: one, or the `SELECT`s
/// of a `UNION ALL` one after the other (`fixtures/sql/cursormore4.json`).
pub(crate) struct LiveCursor {
    parts: Vec<OneCursor>,
    current: usize,
}

impl LiveCursor {
    /// Opens a cursor on `select` that reads its tables live, if it is one
    /// that does.
    pub(crate) fn open(
        session: &Session,
        connection: Handle,
        settings: &Settings,
        select: &Select,
    ) -> Result<Option<LiveCursor>, Error> {
        let mut parts = Vec::new();
        if select.unions.is_empty() {
            parts.extend(OneCursor::open(session, connection, settings, select)?);
        } else if select.unions.iter().all(|(all, _)| *all) && select.order.is_empty() {
            let mut first = select.clone();
            first.unions.clear();
            let all = std::iter::once(&first).chain(select.unions.iter().map(|(_, s)| s));
            for part in all {
                let Some(cursor) = OneCursor::open(session, connection, settings, part)? else {
                    return Ok(None);
                };
                parts.push(cursor);
            }
            // The rows are the first `SELECT`'s columns, which the others'
            // must be as they are for the rows to be read as they come.
            if parts.iter().any(|p| p.types != parts[0].types) {
                return Ok(None);
            }
        }
        Ok((!parts.is_empty()).then_some(LiveCursor { parts, current: 0 }))
    }

    /// The cursor's columns.
    pub(crate) fn columns(&self) -> Vec<Field> {
        self.parts[0].columns()
    }

    /// The next row.
    pub(crate) fn fetch(&mut self) -> Result<Option<Vec<V>>, Error> {
        while let Some(part) = self.parts.get_mut(self.current) {
            if let Some(row) = part.fetch()? {
                return Ok(Some(row));
            }
            self.current += 1;
        }
        Ok(None)
    }
}

/// The table a `SELECT` of a derived table reads, if it is a plain one of a
/// table (or of another such).
fn derived_base(from: &tflip_sql::ast::From) -> Option<&tflip_sql::ast::TableRef> {
    use tflip_sql::ast::From;
    match from {
        From::Table(table) => Some(table),
        From::Derived { select, .. }
            if plain_select(select) && select.top.is_none() && select.order.is_empty() =>
        {
            derived_base(&select.from)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A lookup's bound is the nearest value of the field's precision
    /// inside it (a double's is a little outside), so no record in the
    /// bound is missed.
    #[test]
    fn lookup_bounds_round_into_the_bound() {
        use tflip_storage::adt::field;
        use tflip_storage::field::FieldType;
        let integer = field("i", FieldType::Integer, 5, 4, 0);
        let numeric = field("n", FieldType::Numeric, 9, 8, 2);
        let money = field("m", FieldType::Money, 17, 8, 4);
        let double = field("d", FieldType::Double, 25, 8, 0);
        let date = field("t", FieldType::Date, 33, 4, 0);
        let low = |f: &Field, v: V| bound_value(f, &v, false);
        let high = |f: &Field, v: V| bound_value(f, &v, true);
        assert_eq!(low(&integer, V::Num(25, 1)), Some(Value::Integer(3)));
        assert_eq!(high(&integer, V::Num(25, 1)), Some(Value::Integer(2)));
        assert_eq!(low(&integer, V::Num(-25, 1)), Some(Value::Integer(-2)));
        assert_eq!(high(&integer, V::Num(-25, 1)), Some(Value::Integer(-3)));
        assert_eq!(low(&integer, V::Double(2.5)), Some(Value::Integer(2)));
        assert_eq!(high(&integer, V::Double(2.5)), Some(Value::Integer(3)));
        assert_eq!(low(&integer, V::Int(7)), Some(Value::Integer(7)));
        let text = |s: &str| Some(Value::Numeric(s.to_owned()));
        assert_eq!(low(&numeric, V::Num(25551, 4)), text("2.56"));
        assert_eq!(high(&numeric, V::Num(25551, 4)), text("2.55"));
        assert_eq!(low(&numeric, V::Num(-1555, 3)), text("-1.55"));
        assert_eq!(high(&numeric, V::Num(-1555, 3)), text("-1.56"));
        assert_eq!(low(&numeric, V::Int(-3)), text("-3.00"));
        assert_eq!(low(&numeric, V::Num(5, 2)), text("0.05"));
        assert_eq!(low(&money, V::Num(1, 5)), Some(Value::Money(1)));
        assert_eq!(high(&money, V::Num(1, 5)), Some(Value::Money(0)));
        assert_eq!(
            low(&double, V::Int(1)),
            Some(Value::Double(1f64.next_down()))
        );
        assert_eq!(
            high(&double, V::Int(1)),
            Some(Value::Double(1f64.next_up()))
        );
        assert_eq!(low(&date, V::Date(2_451_545)), Some(Value::Date(2_451_545)));
        assert_eq!(low(&date, V::Str("2000-01-01".to_owned())), None);
    }

    /// Index lookups see the records statements before them wrote.
    #[test]
    fn index_lookups_see_writes() {
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-sql-lookups-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/indexes");
        for file in ["indexed.adt", "indexed.adi"] {
            std::fs::copy(fixtures.join(file), dir.join(file)).unwrap();
        }
        let session = Session::new();
        let connection =
            connection::connect(&session, &format!("Data Source={};", dir.display())).unwrap();
        let statement = create(&session, connection).unwrap();
        let count = |sql: &str| {
            let cursor = execute_direct(&session, statement, sql, true)
                .unwrap()
                .unwrap();
            let count = table::get(&session, cursor)
                .unwrap()
                .record_count()
                .unwrap();
            table::close(&session, cursor).unwrap();
            count
        };
        let by_id = "SELECT id + 0 FROM indexed WHERE id = 77";
        let joined = "SELECT j.name FROM indexed j JOIN indexed i ON i.id = j.id WHERE j.id >= 77";
        assert_eq!((count(by_id), count(joined)), (0, 0));
        let insert = "INSERT INTO indexed (id, name, ciname) VALUES (77, 'Zulu', 'zulu')";
        execute_direct(&session, statement, insert, false).unwrap();
        assert_eq!((count(by_id), count(joined)), (1, 1));
        execute_direct(
            &session,
            statement,
            "UPDATE indexed SET id = 78 WHERE id = 77",
            false,
        )
        .unwrap();
        assert_eq!((count(by_id), count(joined)), (0, 1));
        drop(session);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
