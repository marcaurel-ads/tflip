//! Stored procedures as a statement's tables and arguments.

use super::*;

/// The values of a procedure's arguments.
pub(crate) fn arguments(
    session: &Session,
    connection: Handle,
    execute: &Execute,
    params: &[Param],
) -> Result<Vec<V>, Error> {
    let env = env(
        session,
        connection,
        None,
        params,
        &vec![Ty::Unknown; params.len()],
    )?;
    let mut compiler = tflip_sql::Compiler::new(None, params.len());
    compiler.functions = crate::udf::catalog(session, connection);
    compiler.user = environment::sql_user(session, connection);
    let mut args = Vec::with_capacity(execute.args.len());
    for arg in &execute.args {
        let node = match &arg.kind {
            tflip_sql::ast::ExprKind::Null => {
                args.push(V::Null);
                continue;
            }
            _ => compiler.compile(arg).map_err(Error::Sql)?,
        };
        args.push(tflip_sql::eval(&node, &mut NoRow, &env).map_err(Error::Sql)?);
    }
    Ok(args)
}

/// How [`Opened`] meets the procedures a statement reads in `FROM`.
#[derive(Clone, Copy)]
pub(crate) enum Procedures<'a> {
    /// Runs them, for the statement `statement` (if it has a handle) with
    /// its parameters.
    Run {
        statement: Option<Handle>,
        params: &'a [Param],
    },
    /// Only describes their results' columns, running none: a statement that
    /// is being checked or planned, with this many parameters.
    Describe(usize),
}

impl Procedures<'_> {
    fn params(&self) -> usize {
        match self {
            Procedures::Run { params, .. } => params.len(),
            Procedures::Describe(count) => *count,
        }
    }
}

/// The result of the procedure `execute` calls, as a table of its own that a
/// `FROM` reads as it reads a static cursor (`fixtures/sql/execproc.json`).
pub(super) fn procedure_table(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    execute: &Execute,
    procedures: Procedures<'_>,
) -> Result<table::Table, Error> {
    let name = &execute.name;
    let builtin = procedure::is_builtin(name);
    let signature = match builtin {
        true => None,
        false => Some(
            crate::psm::signature(session, connection, name)?
                .ok_or_else(|| Error::Sql(SqlError::no_procedure(name, execute.pos)))?,
        ),
    };
    let selectable = match &signature {
        Some(signature) => signature.selectable,
        None => procedure::has_results(name),
    };
    if !selectable {
        return Err(Error::Sql(SqlError::not_selectable(name, execute.pos)));
    }
    let wanted = match &signature {
        Some(signature) => Some(signature.inputs.len()),
        None => procedure::arguments(name),
    };
    // No arguments at all is a parameter missing, any other number that
    // is not the procedure's an insert's count.
    match (wanted, execute.args.len()) {
        (Some(wanted), given) if wanted != given => {
            return Err(Error::Sql(match given {
                0 => SqlError::parameter_missing(),
                _ => SqlError::procedure_arguments(),
            }));
        }
        _ => {}
    }
    // Probed for the one that takes none.
    if name.eq_ignore_ascii_case("sp_GetApplicationID") && !execute.args.is_empty() {
        return Err(Error::Sql(SqlError::procedure_parameters(
            name,
            execute.pos,
        )));
    }
    if builtin {
        procedure::refuse_user(session, connection, name)?;
    }
    let builtin_inputs = procedure::inputs(name);
    let inputs = signature
        .as_ref()
        .map_or(&builtin_inputs[..], |s| &s.inputs[..]);
    let args = procedure_arguments(session, connection, settings, execute, inputs, procedures)?;
    let (columns, rows) = match (signature, procedures) {
        (Some(signature), Procedures::Describe(_)) => (signature.columns, Vec::new()),
        (Some(_), Procedures::Run { .. }) => {
            crate::psm::rows(session, connection, settings, name, execute.pos, &args)?
        }
        (None, _) => {
            let owner = match procedures {
                Procedures::Run {
                    statement: Some(statement),
                    ..
                } => statement,
                _ => connection,
            };
            let results = procedure::run(session, owner, connection, execute, &args)?
                .ok_or_else(|| Error::Sql(SqlError::not_selectable(name, execute.pos)))?;
            let rows = match results.shown {
                Some(shown) => shown
                    .iter()
                    .filter_map(|&n| results.rows.get(n as usize - 1).cloned())
                    .collect(),
                None => results.rows,
            };
            (results.columns, rows)
        }
    };
    let adt = build::build(&columns, &rows).map_err(|_| Error::CorruptTable)?;
    table::source(session, connection, name, static_settings(false), adt)
}

/// The values of the arguments of the procedure call `execute`, each checked
/// against the parameter of `inputs` it goes to (a system procedure has none
/// to check); a statement that is only described gets NULL for those that
/// need a parameter's value.
pub(crate) fn procedure_arguments(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    execute: &Execute,
    inputs: &[Field],
    procedures: Procedures<'_>,
) -> Result<Vec<V>, Error> {
    use tflip_sql::compile::Kind;
    let count = procedures.params();
    let mut compiler = tflip_sql::Compiler::new(None, count);
    compiler.functions = crate::udf::catalog(session, connection);
    compiler.user = environment::sql_user(session, connection);
    let mut types = vec![Ty::Unknown; count];
    let mut nodes = Vec::with_capacity(execute.args.len());
    // A call has no row to read a column of: the original engine names the
    // column, not its table, whatever the other tables of the statement.
    for arg in &execute.args {
        let mut missing = None;
        arg.walk(&mut |e| {
            if let tflip_sql::ast::ExprKind::Column { table, name } = &e.kind
                && missing.is_none()
            {
                let at = e.pos + table.as_ref().map_or(0, |t| t.len() + 1);
                missing = Some(SqlError::column_not_found(name, at));
            }
        });
        if let Some(error) = missing {
            return Err(Error::Sql(error));
        }
    }
    for (i, arg) in execute.args.iter().enumerate() {
        let node = match inputs.get(i) {
            Some(input) => {
                let node = compiler
                    .node_for(arg, Ty::of_field(input))
                    .map_err(Error::Sql)?;
                match crate::procedure::is_builtin(&execute.name) {
                    true => tflip_sql::dml::check_argument(input, node.ty, arg.pos),
                    false => tflip_sql::dml::check_user_argument(input, node.ty, arg.pos),
                }
                .map_err(Error::Sql)?;
                Some(node)
            }
            None if matches!(arg.kind, tflip_sql::ast::ExprKind::Null) => None,
            None => Some(compiler.compile(arg).map_err(Error::Sql)?),
        };
        if let Some(Node {
            kind: Kind::Param(n),
            ty,
            ..
        }) = &node
        {
            types[*n] = *ty;
        }
        nodes.push(node);
    }
    let env = match procedures {
        Procedures::Run { params, .. } => env(session, connection, Some(settings), params, &types)?,
        Procedures::Describe(_) => env(session, connection, Some(settings), &[], &[])?,
    };
    let described = matches!(procedures, Procedures::Describe(_));
    nodes
        .into_iter()
        .enumerate()
        .map(|(i, node)| {
            let value = match node {
                None => V::Null,
                Some(node) if described && node.any(&|n| matches!(n.kind, Kind::Param(_))) => {
                    V::Null
                }
                Some(node) => tflip_sql::eval(&node, &mut NoRow, &env).map_err(Error::Sql)?,
            };
            match inputs.get(i) {
                Some(input) if !described => {
                    tflip_sql::script::checked_input(value, input, execute.args[i].pos, &env)
                        .map_err(Error::Sql)
                }
                _ => Ok(value),
            }
        })
        .collect()
}

/// The refusal of a call of the user procedure with `inputs` parameters that
/// gives another number of arguments: arguments to a procedure that takes none
/// are too many, none to one that takes some a parameter missing, any other
/// number that is not its inputs' an insert's count.
pub(crate) fn check_call_arity(inputs: usize, execute: &Execute) -> Result<(), Error> {
    match (inputs, execute.args.len()) {
        (0, 1..) => Err(Error::Sql(SqlError::procedure_parameters(
            &execute.name,
            execute.pos,
        ))),
        (1.., 0) => Err(Error::Sql(SqlError::parameter_missing())),
        (wanted, given) if wanted != given => Err(Error::Sql(SqlError::procedure_arguments())),
        _ => Ok(()),
    }
}
