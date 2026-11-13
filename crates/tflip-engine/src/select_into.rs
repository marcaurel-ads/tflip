//! `SELECT … INTO table`: the result of the statement stored in a new free
//! ADT table, which the statement returns a live cursor on
//! (`fixtures/sql/selinto.json`, the files byte for byte in
//! `fixtures/sql/selinto/`).
//!
//! The table is made and filled as `CREATE TABLE` and `INSERT` would, from the
//! columns the plan gives the result; whatever the statement's table type is,
//! and on a dictionary connection too, it is a free table beside the
//! dictionary's files. A statement that fails leaves no table.
//!
//! `INTO #name` makes the connection's temporary table instead, of the
//! statement's table type as `CREATE TABLE #name` does
//! (`fixtures/sql/temptables_into.json`): a Clipper or FoxPro one with its
//! fields in upper case, which cannot hold what DBF tables cannot, and a
//! Visual FoxPro one whose fields all take NULL
//! (`fixtures/sql/temptables_into2.json`, `temptables_into3.json`,
//! `temptables_into4.json`).

use tflip_sql::SqlError;
use tflip_sql::ast::{From, Item, Named, Select, TableRef};
use tflip_storage::build::Column;
use tflip_storage::create::{self, DbfFieldSpec, FieldSpec};
use tflip_storage::dbf::Flavour;
use tflip_storage::field::FieldType;
use tflip_storage::memo::MemoKind;
use tflip_storage::value::Value;

use crate::handle::Handle;
use crate::statement::{self, Param, Settings};
use crate::table::{self, FieldRef, TableType};
use crate::{Error, Session, connection, ddl};

/// Runs `select`, which names its table `into`, and returns the cursor on that
/// table (none without `want_cursor`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    session: &Session,
    statement: Handle,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    into: &Named,
    params: &[Param],
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    let result = statement::query_rows(
        session,
        statement,
        connection,
        settings,
        select,
        params,
        statement::Keep::Rows,
    )?
    .ok_or(Error::NotImplemented)?;
    let rows = statement::in_sequence(result.rows, result.sequence.as_deref());
    let temporary = crate::temp::is_temp(&into.name);
    // The table is a free ADT one whatever the statement opens, but a
    // temporary table is of the statement's type.
    // (`fixtures/aes/rc4tp` `v16`: under `AdsStmtSetTableType` VFP the file is a
    // VFP table, on any connection.)
    let flavour = ddl::flavour(settings).filter(|&flavour| temporary || flavour == Flavour::Visual);
    let free = match (temporary, flavour) {
        (true, _) | (_, Some(_)) => settings.clone(),
        _ => Settings {
            table_type: TableType::Adt,
            char_type: 1,
            ..settings.clone()
        },
    };
    let dbf_specs = flavour
        .map(|flavour| dbf_specs(select, &result.columns, flavour))
        .transpose()?;
    let path = match temporary {
        true => ddl::temp_path(session, connection, settings, &into.name)?,
        false => {
            let mut path = ddl::path_in(session, connection, &into.name)?;
            if path.extension().is_none() {
                path.set_extension(if flavour.is_some() { "dbf" } else { "adt" });
            }
            if path.exists() {
                return Err(Error::Sql(SqlError::table_exists()));
            }
            path
        }
    };
    let forget = || {
        let _ = std::fs::remove_file(&path);
        for kind in [MemoKind::Adm, MemoKind::Dbt, MemoKind::Fpt] {
            let _ = std::fs::remove_file(tflip_storage::dbf::memo_path(&path, kind));
        }
        if temporary && let Ok(owner) = connection::get(session, connection) {
            owner.temps.remove(&into.name);
        }
    };
    let made = match (&dbf_specs, flavour) {
        (Some(specs), Some(flavour)) => ddl::write_dbf(
            &path,
            flavour,
            specs,
            0,
            settings.char_type,
            flavour == Flavour::Visual && !temporary && ddl::aes_layout(session, connection),
        ),
        _ => {
            let specs: Vec<FieldSpec> = result
                .columns
                .iter()
                .map(|c| FieldSpec {
                    name: c.name.clone(),
                    field_type: c.field_type,
                    length: c.length,
                    decimals: c.decimals,
                    compressed: false,
                })
                .collect();
            // As `CREATE TABLE` lays one out there: for AES, not armed
            // (`fixtures/aes/dictwrite`, `48_select_into`).
            match ddl::lays_out_aes(session, connection)? {
                true => create::aes_adt_table(&path, &specs, ddl::BLOCK_SIZE),
                false => create::adt_table(&path, &specs, ddl::BLOCK_SIZE),
            }
            .map_err(|_| Error::CorruptTable)
        }
    };
    if let Err(error) = made {
        forget();
        return Err(error);
    }
    // A Money the table keeps as a Numeric is written as its digits.
    let rows = match &dbf_specs {
        Some(specs) => rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .zip(specs)
                    .map(|(value, spec)| match value {
                        Value::Money(m) if spec.spec.field_type == FieldType::Numeric => {
                            Value::Numeric(money_digits(m, spec.spec.length))
                        }
                        value => value,
                    })
                    .collect()
            })
            .collect(),
        None => rows,
    };
    let filled = fill(
        session,
        connection,
        &free,
        &into.name,
        &result.columns,
        rows,
    );
    if let Err(error) = filled {
        forget();
        return Err(error);
    }
    let cursor = match want_cursor {
        true => statement::select_cursor_in(
            session,
            statement,
            connection,
            &free,
            &everything(into),
            &[],
            true,
        ),
        false => Ok(None),
    };
    if temporary {
        crate::temp::forget_read(&into.name);
    }
    cursor
}

/// The fields of the DBF table of `flavour` that the result `columns` make,
/// as the original makes them: a Money a Numeric, an AutoInc an Integer, and
/// in a Clipper or FoxPro table a VarChar a Character; a Visual FoxPro field
/// takes NULL, whatever its column did. A name of more than ten characters is
/// refused before the type is, with the definition as the original words it.
fn dbf_specs(
    select: &Select,
    columns: &[Column],
    flavour: Flavour,
) -> Result<Vec<DbfFieldSpec>, Error> {
    let visual = flavour == Flavour::Visual;
    let direct = direct_columns(select, columns.len());
    let mut specs = Vec::with_capacity(columns.len());
    for (column, direct) in columns.iter().zip(direct) {
        let name = match visual {
            true => column.name.clone(),
            false => column.name.to_ascii_uppercase(),
        };
        // The plan gives a column of a table a Numeric's length in an ADT
        // table's terms, a digit more than the DBF field has, which the
        // table the original makes keeps.
        let (n, d) = (
            column.length - u16::from(direct && column.field_type == FieldType::Numeric),
            column.decimals,
        );
        let (made, shown) = match column.field_type {
            FieldType::Logical => ("Logical".to_owned(), "L".to_owned()),
            FieldType::Numeric => (format!("Numeric,{n},{d}"), format!("N,{n},{d}")),
            // A column of a table is 23 wide, a Money expression's value 21.
            FieldType::Money => {
                let n = if direct { 23 } else { 21 };
                (format!("Numeric,{n},4"), format!("N,{n},4"))
            }
            FieldType::Date | FieldType::CompactDate => ("Date".to_owned(), "D".to_owned()),
            FieldType::Character | FieldType::CiCharacter => {
                (format!("Character,{n}"), format!("CH,{n},0"))
            }
            FieldType::VarChar if visual => (format!("VarChar,{n}"), format!("VARCHARFOX,{n},0")),
            FieldType::VarChar => (format!("Character,{n}"), format!("CH,{n},0")),
            FieldType::Memo => ("Memo".to_owned(), "M".to_owned()),
            FieldType::Binary | FieldType::Image => ("Binary".to_owned(), "B".to_owned()),
            FieldType::NMemo => ("NMemo".to_owned(), "NMEMO".to_owned()),
            FieldType::Double | FieldType::CurDouble => {
                (format!("Double,{d}"), format!("DOUBLE,{d}"))
            }
            FieldType::Integer | FieldType::AutoInc => ("Integer".to_owned(), "I".to_owned()),
            // A Visual FoxPro table has no short integers, the others take
            // them as integers.
            FieldType::ShortInt if !visual => ("Integer".to_owned(), "I".to_owned()),
            FieldType::LongInt => {
                let shown = if visual { "LongInt" } else { "LONGINT" };
                ("LongInt".to_owned(), shown.to_owned())
            }
            FieldType::ShortInt => ("ShortInt".to_owned(), "SHORT".to_owned()),
            FieldType::Time => ("Time".to_owned(), "TIME".to_owned()),
            FieldType::TimeStamp | FieldType::ModTime => {
                ("TimeStamp".to_owned(), "TIMESTAMP".to_owned())
            }
            FieldType::Raw => (format!("Raw,{n}"), format!("RAW,{n},0")),
            FieldType::VarBinary => (format!("VarBinary,{n}"), format!("VARBINARYFOX,{n},0")),
            FieldType::NChar => (format!("NChar,{n}"), format!("NCHAR,{n},0")),
            FieldType::NVarChar => (format!("NVarChar,{n}"), format!("NVARCHAR,{n},0")),
            FieldType::Guid => ("GUID".to_owned(), "GUID".to_owned()),
            _ => return Err(Error::NotImplemented),
        };
        let null = if visual { ",NULL" } else { "" };
        if name.chars().count() > 10 {
            return Err(ddl::invalid_field(&format!(
                "Field name invalid.  The problem field definition is: \"{name},{shown}{null};\"."
            )));
        }
        let field = ddl::dbf_field_specs(&format!("{name},{made}{null}"), flavour);
        specs.extend(field.map_err(|error| match error {
            Error::InvalidFieldDefinition(why) => ddl::invalid_field(&why),
            error => error,
        })?);
    }
    Ok(specs)
}

/// A Money's `value` × 10,000 as the digits of a Numeric field of `width`.
fn money_digits(value: i64, width: u16) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let text = format!(
        "{sign}{}.{:04}",
        value.unsigned_abs() / 10_000,
        value.unsigned_abs() % 10_000
    );
    format!("{text:>width$}", width = usize::from(width))
}

/// Which of the `count` result columns of `select` are columns of its tables,
/// as `*` and a name are: the others are expressions.
fn direct_columns(select: &Select, count: usize) -> Vec<bool> {
    let stars = select
        .items
        .iter()
        .filter(|item| matches!(item, Item::Star { .. }))
        .count();
    // A star's width is what the other items leave, if it is the only one.
    let width = count.saturating_sub(select.items.len() - stars);
    let mut direct = Vec::with_capacity(count);
    for item in &select.items {
        match item {
            Item::Star { .. } => direct.extend(std::iter::repeat_n(true, width)),
            Item::Expr { expr, .. } => {
                direct.push(matches!(expr.kind, tflip_sql::ast::ExprKind::Column { .. }))
            }
        }
    }
    direct.resize(count, false);
    direct
}

/// Appends `rows` to the table `name`.
fn fill(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
    columns: &[tflip_storage::build::Column],
    rows: Vec<Vec<Value>>,
) -> Result<(), Error> {
    let handle = table::open(session, connection, name, settings.table_named(0, name))?;
    let written = table::get(session, handle).and_then(|target| {
        for row in rows {
            target.append()?;
            for (i, (value, column)) in row.into_iter().zip(columns).enumerate() {
                let value = crate::dml::set_value_in(column.field_type, value);
                target.set_field(FieldRef::Number(i + 1), &value)?;
            }
            target.write_record()?;
        }
        Ok(())
    });
    let closed = table::close(session, handle);
    written.and(closed)
}

/// `SELECT * FROM <into>`: the cursor a statement returns.
fn everything(into: &Named) -> Select {
    Select {
        force_static: false,
        top: None,
        distinct: false,
        items: vec![Item::Star {
            table: None,
            pos: 0,
        }],
        into: None,
        from: From::Table(TableRef {
            name: into.name.clone(),
            link: None,
            alias: None,
            pos: into.pos,
            alias_pos: into.pos,
            id: 0,
        }),
        filter: None,
        group: Vec::new(),
        having: None,
        unions: Vec::new(),
        order: Vec::new(),
    }
}
