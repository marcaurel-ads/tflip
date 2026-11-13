//! The columns `sp_GetColumns` lists for a dictionary's tables and views.
//!
//! A table's columns are its fields, with what the dictionary says of each:
//! its comment, default, options and whether it is `NOT NULL`. A view's are
//! the columns of the cursor its `SELECT` opens, typed as the cursor types
//! them, and the original engine works out their nullability from the
//! `SELECT` (`fixtures/sql/spview.json`):
//!
//! - a column of a table (or of a view) in the select list, bare or in
//!   parentheses, is that column, with its nullability, and its comment,
//!   default and options carried through — even from the nullable side of an
//!   outer join;
//! - anything else is nullable when a column in it is, except that a
//!   constant `NULL`, a parameter, a subquery, `IS NULL`, `CASE` and `IIF`
//!   always are, `COUNT` and `EXISTS` never, and the other aggregates always;
//! - after `UNION`, a column is the first `SELECT`'s, but a table column's
//!   dictionary properties only carry through if the union kept its type
//!   and length.
//!
//! A view that reads a `SELECT` in its `FROM` cannot be described: the
//! original engine fails with 5112.

use tflip_sql::SqlError;
use tflip_sql::ast::{Aggregate, Expr, ExprKind, From, Item, Select, Statement};
use tflip_storage::dd::{self, Kind, Object};
use tflip_storage::field::Field;
use tflip_storage::table::{Format, Structure};

use crate::dictionary::Database;
use crate::handle::Handle;
use crate::{Error, Session, connection, statement, table};

/// What a dictionary says of a table's column.
#[derive(Debug, Clone, Default)]
pub(crate) struct Meta {
    pub comment: Option<String>,
    /// The default, as text.
    pub default: Option<String>,
    /// `ADS_DD_FIELD_OPTIONS`.
    pub options: u32,
}

/// A column as `sp_GetColumns` describes it.
#[derive(Debug, Clone)]
pub(crate) struct Described {
    pub field: Field,
    pub nullable: bool,
    /// The dictionary's properties of the table column it is, if it is one.
    pub meta: Option<Meta>,
}

/// How deep views may read views before the ones further in go undescribed.
const MAX_DEPTH: usize = 16;

/// The columns of the dictionary table `object`, with the dictionary's
/// properties of each; `None` if its file cannot be read.
pub(crate) fn table_columns(db: &Database, object: &Object) -> Option<Vec<Described>> {
    let bound = db.table(&object.name)?;
    let structure = Structure::read(&bound.path, Format::Adt).ok()?;
    let fields = db.dictionary().children(object.id, Kind::Field);
    let columns = structure
        .fields
        .into_iter()
        .map(|field| {
            let found = fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&field.name));
            let not_null = found.is_some_and(|f| f.has(dd::field::NOT_NULL));
            let meta = Meta {
                comment: found.and_then(|f| f.comment.clone()),
                default: found.and_then(|f| f.text(dd::field::DEFAULT_VALUE)),
                options: found.and_then(|f| f.u32(dd::field::OPTIONS)).unwrap_or(0),
            };
            // A column is nullable when its type allows it and the
            // dictionary does not mark it NOT NULL.
            let nullable = field.field_type.nullable() && !not_null;
            Described {
                field,
                nullable,
                meta: Some(meta),
            }
        })
        .collect();
    Some(columns)
}

/// The columns of the view `name` of `connection`'s dictionary.
pub(crate) fn view_columns(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<Vec<Described>, Error> {
    describe_view(session, connection, name, 0)
}

fn describe_view(
    session: &Session,
    connection: Handle,
    name: &str,
    depth: usize,
) -> Result<Vec<Described>, Error> {
    let connect = connection::get(session, connection)?;
    let sql = connect
        .with_database_opt(|db| db.view(name))
        .flatten()
        .ok_or(Error::InvalidObjectName)?;
    let parsed = tflip_sql::parse(&sql).map_err(Error::Sql)?;
    let Statement::Select(select) = &parsed.statement else {
        return Err(Error::InvalidObjectName);
    };
    if reads_derived(select) {
        return Err(Error::Sql(SqlError::ace_bare(
            5112,
            "The column number specified was invalid for the cursor.",
        )));
    }
    let fields = cursor_fields(session, connection, &sql)?;
    let mut sources = Vec::new();
    add_sources(session, connection, &select.from, depth, &mut sources)?;
    let items = items(select, &sources);
    let union = !select.unions.is_empty();
    Ok(fields
        .into_iter()
        .enumerate()
        .map(|(i, field)| match items.get(i) {
            Some(Column::Of(base)) => {
                // A union that changed the column's type is not the column.
                let kept = !union
                    || (base.field.field_type == field.field_type
                        && base.field.length() == field.length());
                let meta = base.meta.clone().filter(|_| kept);
                Described {
                    field,
                    nullable: base.nullable,
                    meta,
                }
            }
            Some(&Column::Computed(nullable)) => Described {
                field,
                nullable,
                meta: None,
            },
            None => {
                let nullable = field.field_type.nullable();
                Described {
                    field,
                    nullable,
                    meta: None,
                }
            }
        })
        .collect())
}

/// Whether `select`, or a `SELECT` it unions, reads a `SELECT` in its `FROM`.
fn reads_derived(select: &Select) -> bool {
    fn derived(from: &From) -> bool {
        match from {
            From::Table(_) => false,
            From::Derived { .. } | From::Procedure { .. } => true,
            From::Join { left, right, .. } => derived(left) || derived(right),
        }
    }
    derived(&select.from) || select.unions.iter().any(|(_, s)| reads_derived(s))
}

/// The fields of the cursor the view's `sql` opens on `connection`.
fn cursor_fields(session: &Session, connection: Handle, sql: &str) -> Result<Vec<Field>, Error> {
    let cursor = statement::open_view(session, connection, sql, "")?;
    let (fields, owner) = {
        let table = table::get(session, cursor)?;
        (table.fields().to_vec(), table.statement())
    };
    let _ = table::close(session, cursor);
    if let Some(owner) = owner {
        let _ = statement::close(session, owner);
    }
    Ok(fields)
}

/// A table or view a view reads, and its columns.
struct Source {
    name: String,
    alias: Option<String>,
    columns: Vec<Described>,
}

/// Adds the tables and views `from` reads, in order, to `sources`.
fn add_sources(
    session: &Session,
    connection: Handle,
    from: &From,
    depth: usize,
    sources: &mut Vec<Source>,
) -> Result<(), Error> {
    match from {
        From::Table(table) => {
            // A linked dictionary's table is described by that dictionary.
            let connection = match &table.link {
                Some(link) => crate::link::open(session, connection, link, &table.name)?,
                None => connection,
            };
            let connect = connection::get(session, connection)?;
            let (columns, view) = connect
                .with_database_opt(|db| match db.dictionary().named(Kind::Table, &table.name) {
                    Some(object) => (table_columns(db, object), false),
                    None => (None, db.view(&table.name).is_some()),
                })
                .unwrap_or((None, false));
            let columns = match (columns, view) {
                (Some(columns), _) => columns,
                (None, true) if depth < MAX_DEPTH => {
                    describe_view(session, connection, &table.name, depth + 1)?
                }
                // A free table, a system table: nothing is carried through.
                _ => Vec::new(),
            };
            sources.push(Source {
                name: table.name.clone(),
                alias: table.alias.clone(),
                columns,
            });
        }
        From::Join { left, right, .. } => {
            add_sources(session, connection, left, depth, sources)?;
            add_sources(session, connection, right, depth, sources)?;
        }
        From::Derived { .. } | From::Procedure { .. } => {}
    }
    Ok(())
}

/// A column of a view's select list: a column it reads, or an expression
/// that is nullable or not.
enum Column {
    Of(Described),
    Computed(bool),
}

/// The select list of `select` (its first `SELECT`'s, for a union), with
/// each `*` expanded.
fn items(select: &Select, sources: &[Source]) -> Vec<Column> {
    let mut columns = Vec::new();
    for item in &select.items {
        match item {
            Item::Star { table, .. } => {
                for source in sources {
                    if table.as_deref().is_none_or(|t| names(source, t)) {
                        columns.extend(source.columns.iter().cloned().map(Column::Of));
                    }
                }
            }
            Item::Expr { expr, .. } => columns.push(match &expr.kind {
                ExprKind::Column { table, name } => {
                    match resolve(sources, table.as_deref(), name) {
                        Some(base) => Column::Of(base.clone()),
                        None => Column::Computed(true),
                    }
                }
                _ => Column::Computed(nullable(expr, sources)),
            }),
        }
    }
    columns
}

/// Whether `source` is called `name`: its alias, or its table's name.
fn names(source: &Source, name: &str) -> bool {
    match &source.alias {
        Some(alias) => alias.eq_ignore_ascii_case(name),
        None => source.name.eq_ignore_ascii_case(name),
    }
}

/// The column `name` (of the table `table`, if qualified) of `sources`.
fn resolve<'a>(sources: &'a [Source], table: Option<&str>, name: &str) -> Option<&'a Described> {
    sources
        .iter()
        .filter(|source| table.is_none_or(|t| names(source, t)))
        .find_map(|source| {
            source
                .columns
                .iter()
                .find(|c| c.field.name.eq_ignore_ascii_case(name))
        })
}

/// Whether the original engine calls the expression `expr` of a view's
/// select list nullable.
fn nullable(expr: &Expr, sources: &[Source]) -> bool {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Typed(..) => false,
        ExprKind::Null | ExprKind::Default | ExprKind::Param(_) | ExprKind::Subquery(_) => true,
        ExprKind::IsNull { .. } | ExprKind::Case { .. } => true,
        ExprKind::Func { name, .. } if name.eq_ignore_ascii_case("IIF") => true,
        ExprKind::Exists(_) => false,
        ExprKind::Aggregate { func, .. } => *func != Aggregate::Count,
        ExprKind::Column { table, name } => {
            resolve(sources, table.as_deref(), name).is_none_or(|c| c.nullable)
        }
        _ => {
            let mut any = false;
            expr.for_each_child(&mut |e| any |= nullable(e, sources));
            any
        }
    }
}
