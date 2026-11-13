//! User-defined functions of a dictionary, called from SQL expressions
//! (`sq(3)`, `util.pad(id, 4)`).
//!
//! A function's body is SQL/PSM like a stored procedure's, but it returns a
//! value with `RETURN` and its parameters are read by their plain names (a
//! stored procedure reads them as `_name`). The compiler resolves a call to a
//! function the dictionary holds ([`catalog`]) and the evaluator runs it
//! ([`functions`]): the parameters are bound as variables, the body runs
//! through the interpreter, and the `RETURN` value is coerced to the declared
//! return type. The statements of the body run on the connection of the
//! statement that calls the function, as a procedure's do: it writes, reads
//! with subqueries, and counts the rows it changed with those of the other
//! functions of the statement (`::stmt.UpdateCount`, `fixtures/sql/udf.json`).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use tflip_sql::ast::{Expr, ScriptStmt, Statement};
use tflip_sql::script::{self, Vars};
use tflip_sql::{Env, FunctionSig, SqlError, Ty, UserFunctions, V};
use tflip_storage::dd::{Kind, Object, function};

use crate::handle::Handle;
use crate::script_host::{ScriptCtx, SettingsFrom};
use crate::statement::Settings;
use crate::{Session, connection};

/// A function of the dictionary, ready to run.
#[derive(Debug, Clone)]
struct Def {
    /// The call name: `sq`, or `util.pad` for a packaged function.
    call_name: String,
    /// The return type as written (`INTEGER`, `STRING`).
    return_type: String,
    /// Each parameter's name and type as written.
    params: Vec<(String, String)>,
    body: Vec<ScriptStmt>,
    /// The function's object in the dictionary, and its package's.
    object: u32,
    package: Option<u32>,
}

/// The user-defined functions of `connection`'s dictionary, as the signatures
/// the compiler resolves calls against. Empty for a folder connection or one
/// whose functions do not parse.
pub fn catalog(session: &Session, connection: Handle) -> Vec<FunctionSig> {
    signatures(&defs(session, connection))
}

/// The signatures of the functions `defs`.
fn signatures(defs: &[Def]) -> Vec<FunctionSig> {
    defs.iter()
        .filter_map(|def| {
            let field = script::declared_field("", &def.return_type).ok()?;
            let params = def
                .params
                .iter()
                .map(|(name, ty)| Some(Ty::of_field(&script::declared_field(name, ty).ok()?)))
                .collect::<Option<Vec<Ty>>>()?;
            Some(FunctionSig {
                name: def.call_name.clone(),
                ret: Ty::of_field(&field),
                arity: def.params.len(),
                params,
            })
        })
        .collect()
}

/// The user-defined functions of `connection`'s dictionary, ready to run, for
/// an evaluation environment; `None` for a folder connection.
pub fn functions(
    session: &Session,
    connection: Handle,
    settings: Option<&Settings>,
) -> Option<Arc<dyn UserFunctions>> {
    let defs = defs(session, connection);
    if defs.is_empty() {
        return None;
    }
    let now = crate::clock::now();
    let (date_format, epoch, database, rand, collation) = connection::get(session, connection)
        .map(|c| {
            (
                c.date_format(),
                c.epoch(),
                c.path.display().to_string(),
                Arc::clone(&c.rand),
                crate::statement::collation(session, &c, ""),
            )
        })
        .unwrap_or_default();
    let case_fold = collation.as_ref().map(|c| c.case_fold());
    let udf = Arc::new_cyclic(|me| Udf {
        defs,
        date_format,
        epoch,
        now: now.local,
        case_fold,
        collation,
        database,
        rand,
        me: me.clone(),
        session: session.clone(),
        connection,
        settings: settings.cloned().unwrap_or_default(),
        updated: AtomicU32::new(0),
    });
    Some(Arc::new(Checked(udf)))
}

/// The functions a statement calls: each call is held to the rights of the
/// user, which the calls the functions make among themselves are not (the
/// body runs with its owner's, `fixtures/sql/ddexecrights`).
#[derive(Debug)]
struct Checked(Arc<Udf>);

impl UserFunctions for Checked {
    fn call(&self, name: &str, args: &[V]) -> Result<V, SqlError> {
        if let Some(def) = self.0.find(name, args.len()) {
            self.0.refuse(def, !crate::psm::running())?;
        }
        self.0.call(name, args)
    }
}

/// The function definitions of `connection`'s dictionary that parse.
fn defs(session: &Session, connection: Handle) -> Vec<Def> {
    let Ok(owner) = connection::get(session, connection) else {
        return Vec::new();
    };
    owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            dict.all(Kind::Function)
                .iter()
                .filter_map(|object| def(dict, object))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// A function object as a [`Def`], if its body parses.
fn def(dict: &tflip_storage::dd::Dictionary, object: &Object) -> Option<Def> {
    let body = object.text(function::SCRIPT).unwrap_or_default();
    let body = tflip_sql::parse::parse_script(&body).ok()?;
    let package = dict
        .object(object.parent)
        .filter(|p| p.kind == Kind::Package);
    let call_name = match package {
        Some(package) => format!("{}.{}", package.name, object.name),
        None => object.name.clone(),
    };
    Some(Def {
        call_name,
        return_type: object.text(function::RETURN_TYPE).unwrap_or_default(),
        params: function_params(&object.text(function::PARAMETERS).unwrap_or_default()),
        body,
        object: object.id,
        package: package.map(|p| p.id),
    })
}

/// The parameters in a function's stored `PARAMETERS` (`i INTEGER, len
/// INTEGER`): each a name and its type as written.
fn function_params(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .filter(|part| !part.trim().is_empty())
        .filter_map(|part| {
            let part = part.trim();
            let (name, ty) = part.split_once(char::is_whitespace)?;
            Some((name.trim().to_owned(), ty.trim().to_owned()))
        })
        .collect()
}

/// The dictionary's functions, ready to run.
#[derive(Debug)]
struct Udf {
    defs: Vec<Def>,
    date_format: String,
    epoch: u16,
    now: (i32, u32),
    /// How `UPPER`/`LOWER` fold case under `collation`.
    case_fold: Option<tflip_sql::CaseFold>,
    /// The connection's collation for SQL comparisons.
    collation: Option<Arc<tflip_sql::Collation>>,
    /// The connection's database path, for `DATABASE()`.
    database: String,
    /// The connection's generator, for `RAND()`.
    rand: Arc<std::sync::Mutex<tflip_sql::Rand>>,
    /// A handle to itself, so a function body can call another function.
    me: Weak<Udf>,
    /// The session and connection the statements of the functions run on, with
    /// the settings of the statement that calls them.
    session: Session,
    connection: Handle,
    settings: Settings,
    /// The rows they changed so far, together.
    updated: AtomicU32,
}

impl Udf {
    fn find(&self, name: &str, arity: usize) -> Option<&Def> {
        (self.defs.iter())
            .find(|d| d.call_name.eq_ignore_ascii_case(name) && d.params.len() == arity)
    }

    /// Fails the call of `def` (5054) where the original does: a `DB:Public`
    /// rights slot that does not check, of the function or its package, in a
    /// dictionary that verifies rights, for the administrator too; and with
    /// `rights`, the user's not holding the right to execute it (the
    /// package's, for a packaged function).
    fn refuse(&self, def: &Def, rights: bool) -> Result<(), SqlError> {
        use crate::dictionary::ExecuteRefusal;
        let Ok(owner) = connection::get(&self.session, self.connection) else {
            return Ok(());
        };
        let refusal = owner.with_database_opt(|db| {
            let dict = db.dictionary();
            let function = dict.object(def.object)?;
            let package = def.package.and_then(|id| dict.object(id));
            let bad = db.verifies_rights()
                && (db.public_slot_bad(function) || package.is_some_and(|p| db.public_slot_bad(p)));
            if bad {
                return Some(ExecuteRefusal::BadSlot);
            }
            match rights {
                true => db.execute_refusal(package.unwrap_or(function)),
                false => None,
            }
        });
        let text = crate::ace_error::error_text(5054).unwrap_or_default();
        match refusal.flatten() {
            Some(ExecuteRefusal::BadSlot) => Err(SqlError::ace_bare(5054, text)),
            Some(ExecuteRefusal::NotHeld) => Err(SqlError::ace(
                5054,
                text,
                match def.package {
                    Some(_) => "The user does not have permission to use the package.",
                    None => "The user does not have permission to use the function.",
                },
            )),
            None => Ok(()),
        }
    }
}

impl UserFunctions for Udf {
    fn call(&self, name: &str, args: &[V]) -> Result<V, SqlError> {
        let def = self
            .find(name, args.len())
            .ok_or_else(|| SqlError::unsupported(&format!("the function {name}"), 0))?;
        self.refuse(def, false)?;
        // A function reads its parameters by their plain names.
        let mut vars = Vars::new();
        for ((pname, ty), value) in def.params.iter().zip(args) {
            vars.define(pname, ty, value.clone())?;
        }
        let env = Env {
            date_format: self.date_format.clone(),
            epoch: self.epoch,
            now: self.now,
            utc_now: self.now,
            params: Vec::new(),
            functions: self.me.upgrade().map(|me| me as Arc<dyn UserFunctions>),
            case_fold: self.case_fold.clone(),
            collation: self.collation.clone(),
            database: self.database.clone(),
            rand: Arc::clone(&self.rand),
            ..Env::default()
        };
        let mut host = Host::new(self);
        let flow = script::run(&def.body, &mut vars, &mut host, &env).map_err(|mut error| {
            // What fails in a script says which function it ran in.
            if error.message.contains(" -- Location of error") {
                error
                    .message
                    .push_str(&format!(" Error in UDF: {}", def.call_name));
            }
            error
        })?;
        let value = match flow {
            script::Flow::Return(Some(value)) => value,
            _ => V::Null,
        };
        Ok(coerce(value, &def.return_type))
    }
}

/// Coerces `value` to the declared return type `ty`, as the engine returns it.
fn coerce(value: V, ty: &str) -> V {
    match script::declared_field("", ty) {
        Ok(field) => script::coerced(value, &field),
        Err(_) => value,
    }
}

/// The host a function body's statements run through: the connection of the
/// statement that calls it.
struct Host<'a> {
    udf: &'a Udf,
    ctx: ScriptCtx<'a>,
}

impl<'a> Host<'a> {
    fn new(udf: &'a Udf) -> Self {
        let settings = SettingsFrom::Given(&udf.settings);
        Host {
            udf,
            ctx: ScriptCtx::new(&udf.session, udf.connection, settings).function(),
        }
    }

    /// What the body's statements read of themselves and the connection.
    fn scope(&self) -> crate::sysvars::Scope<'_> {
        self.ctx
            .scope(None, self.udf.updated.load(Ordering::Relaxed), None)
    }
}

impl script::Host for Host<'_> {
    fn functions(&self) -> Vec<FunctionSig> {
        signatures(&self.udf.defs)
    }

    /// What a function's evaluation fails with is not located.
    fn locates_errors(&self) -> bool {
        false
    }

    fn rewrite_expr(&self, expr: &Expr, vars: &Vars) -> Result<Option<Expr>, SqlError> {
        self.ctx.rewrite_expr(&self.scope(), expr, vars)
    }

    fn run(&mut self, statement: &Statement, vars: &Vars) -> Result<(), SqlError> {
        let statement = self.scope().statement(&script::substitute(statement, vars));
        match &statement {
            Statement::Insert(_)
            | Statement::Update(_)
            | Statement::Delete(_)
            | Statement::Merge(_) => {
                let rows = self.ctx.dml(&statement)?;
                self.udf.updated.fetch_add(rows, Ordering::Relaxed);
            }
            // A bare `SELECT` has no result to return, but is checked.
            Statement::Select(select) => self.ctx.check_select(select)?,
            _ => {
                return Err(SqlError::unsupported(
                    "this statement in a user-defined function",
                    0,
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tflip_storage::value::Value;

    use crate::handle::Handle;
    use crate::script_host::tests::{open as open_psm, query};
    use crate::{Session, statement, table};

    /// A fresh copy of the `psm` fixture in a directory of its own (`name`
    /// keeps concurrent tests apart), with one connection and statement on it.
    fn open(name: &str) -> (Session, Handle) {
        open_psm("udf", name)
    }

    /// Runs `sql` and returns its rows, closing the cursor after.
    fn rows(session: &Session, stmt: Handle, sql: &str) -> Vec<Vec<Value>> {
        query(session, stmt, sql).1
    }

    fn text(value: &Value) -> String {
        match value {
            Value::Text(s) => s.trim_end().to_owned(),
            Value::Integer(n) => n.to_string(),
            other => panic!("not text/int: {other:?}"),
        }
    }

    #[test]
    fn calls_functions_in_a_select() {
        // Matches `adsdump`: sq(id) = id*id, util.pad(id, 4) = zero-padded.
        let (session, stmt) = open("select");
        let out = rows(
            &session,
            stmt,
            "SELECT id, sq(id) AS s, util.pad(id, 4) AS p FROM items",
        );
        let cells: Vec<(String, String, String)> = out
            .iter()
            .map(|r| (text(&r[0]), text(&r[1]), text(&r[2])))
            .collect();
        assert_eq!(
            cells,
            [
                ("1".to_owned(), "1".to_owned(), "0001".to_owned()),
                ("2".to_owned(), "4".to_owned(), "0002".to_owned()),
            ]
        );
    }

    #[test]
    fn calls_functions_with_constant_arguments() {
        let (session, stmt) = open("const");
        let out = rows(
            &session,
            stmt,
            "SELECT sq(5) AS s, util.pad(7, 4) AS p FROM system.iota",
        );
        assert_eq!(text(&out[0][0]), "25");
        assert_eq!(text(&out[0][1]), "0007");
    }

    #[test]
    fn a_function_is_an_argument_to_a_procedure() {
        // add_item(50, 'x', sq(3)): the qty is sq(3) = 9.
        let (session, stmt) = open("arg");
        let cursor = statement::execute_direct(
            &session,
            stmt,
            "EXECUTE PROCEDURE add_item(50, 'x', sq(3))",
            true,
        )
        .unwrap();
        if let Some(cursor) = cursor {
            table::close(&session, cursor).unwrap();
        }
        // MAX forces a static cursor, so the scan sees the filtered result
        // (a live cursor's WHERE is applied by navigation, not by `scan`).
        let out = rows(
            &session,
            stmt,
            "SELECT MAX(qty) AS q FROM items WHERE id = 50",
        );
        assert_eq!(text(&out[0][0]), "9", "rows={out:?}");
    }
}
