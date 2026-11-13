//! The system variables a statement reads: `::conn.*` of its connection,
//! `::stmt.*` of itself and the trigger it runs in, and `::server.*`
//! (`fixtures/sql/sysvars.json`, `trigvars.json`, `procvars.json`).
//!
//! A host puts each one in the statement as a literal of its type before the
//! statement runs ([`Scope::statement`]), and in the expressions of a script
//! the same way ([`Scope::expr`]); a name that is no variable is left as
//! it is, for the compiler to refuse.

use tflip_sql::V;
use tflip_sql::ast::{CastType, Expr, ExprKind, Item, Select, Statement as Sql};
use tflip_sql::script;

use crate::handle::Handle;
use crate::{Session, connection, statement};

/// The trigger a script runs in, which `::stmt.Trig*` tell of.
#[derive(Debug, Clone)]
pub struct Trigger {
    pub name: String,
    pub table: String,
    /// `ADS_TRIGEVENT_INSERT` 1, `UPDATE` 2, `DELETE` 3.
    pub event: u32,
    /// `ADS_TRIGTYPE_BEFORE` 1, `INSTEADOF` 2, `AFTER` 4.
    pub kind: u32,
    /// The `ROWID` of the record the trigger fires on.
    pub rowid: String,
    pub recno: u32,
}

/// What a statement of a script can ask of the engine about itself.
pub struct Scope<'a> {
    pub session: &'a Session,
    pub connection: Handle,
    /// The statement the script is run by, if there is one (a trigger
    /// is run by the statement that changed the table).
    pub statement: Option<Handle>,
    /// How many rows the script's statements changed so far
    /// (`::stmt.UpdateCount`).
    pub updated: u32,
    pub trigger: Option<&'a Trigger>,
}

/// The length of the text of `::conn.Name` that names the connection's
/// folder or dictionary.
const NAME_PATH: usize = 30;

impl Scope<'_> {
    /// `statement` with the system variables in it as literals, and a column of
    /// a `SELECT` that is one named after it.
    pub fn statement(&self, statement: &Sql) -> Sql {
        let mut statement = statement.clone();
        if let Sql::Select(select) = &mut statement {
            self.name_items(select);
            for (_, union) in &mut select.unions {
                self.name_items(union);
            }
        }
        script::rewrite(&statement, &|kind| self.literal(kind))
    }

    /// The columns of `select` that are a variable by itself, as they
    /// are named after it.
    fn name_items(&self, select: &mut Select) {
        for item in &mut select.items {
            if let Item::Expr { expr, alias } = item
                && alias.is_none()
                && let ExprKind::Column { table: None, name } = &expr.kind
                && name.starts_with("::")
                && let Some((_, last)) = name.rsplit_once('.')
                && self.value(name).is_some()
            {
                *alias = Some(last.to_owned());
            }
        }
    }

    /// `expr`, an expression of the script, with them in it.
    pub fn expr(&self, expr: &Expr) -> Expr {
        let mut out = script::rewrite_expr(expr, &|kind| self.literal(kind));
        // A variable that is the whole expression is where its name is, for
        // the errors of an assignment of it.
        if let ExprKind::Column { table: None, name } = &expr.kind
            && let Some((scope, _)) = name.split_once('.')
            && out.kind != expr.kind
        {
            out.pos += scope.chars().count() + 1;
        }
        out
    }

    /// The literal for `kind` if it is a system variable.
    pub fn literal(&self, kind: &ExprKind) -> Option<ExprKind> {
        match kind {
            ExprKind::Column { table: None, name } => {
                let (value, to) = self.value(name)?;
                Some(script::typed(&value, to))
            }
            _ => None,
        }
    }

    /// The value of the variable `name` (`::conn.Name`, in any case) and the
    /// type a statement reads it as; `None` if it is none.
    pub fn value(&self, name: &str) -> Option<(V, CastType)> {
        let (scope, name) = name.strip_prefix("::")?.split_once('.')?;
        let owner = connection::get(self.session, self.connection).ok()?;
        let text = |s: &str| match s.is_empty() {
            true => (V::Str(String::new()), CastType::VarChar(Some(1))),
            false => (
                V::Str(s.to_owned()),
                CastType::VarChar(Some(s.chars().count().min(65530) as u16)),
            ),
        };
        let number = |n: u32| (V::Int(i64::from(n)), CastType::Integer);
        let null_text = |length| (V::Null, CastType::VarChar(Some(length)));
        let null_number = || (V::Null, CastType::Integer);
        let on = |what: &str| name.eq_ignore_ascii_case(what);
        Some(match scope.to_ascii_lowercase().as_str() {
            "conn" if on("Name") => {
                let path = owner.path.display().to_string();
                let count = path.chars().count();
                let path = match count > NAME_PATH - 1 {
                    // A long path is cut in the middle.
                    true => {
                        let chars: Vec<char> = path.chars().collect();
                        let head: String = chars[..15].iter().collect();
                        let tail: String = chars[count - 12..].iter().collect();
                        format!("{head}..{tail}")
                    }
                    false => path,
                };
                let name = format!(
                    "CONN{:08X}{:08X}{path:_>width$}",
                    owner.number + 2,
                    owner.number,
                    width = NAME_PATH
                );
                (V::Str(name), CastType::Char(Some(50)))
            }
            "conn" if on("OperationCount") => {
                number(owner.operations.load(std::sync::atomic::Ordering::Relaxed))
            }
            "conn" if on("OSUserLoginName") => text(&crate::connection::os_user()),
            // The client's address as a server saw it; its host name is
            // not sent (KNOWN_ISSUES.md).
            "conn" if on("NetworkAddress") => {
                let address = crate::lock(&self.session.client_address).clone();
                text(&address.unwrap_or_default())
            }
            "conn" if on("ClientHostName") || on("TerminalClientAddress") => text(""),
            "conn" if on("IsRoot") => (V::Bool(false), CastType::Bit),
            "conn" if on("TransactionCount") => {
                number(crate::transaction::count(self.session, self.connection).unwrap_or(0))
            }
            "conn" if on("Collation") => {
                let collation = owner.collation();
                let collation = match collation.is_empty() {
                    true => "default_ansi".to_owned(),
                    false => collation,
                };
                (V::Str(collation), CastType::VarChar(Some(100)))
            }
            "stmt" if on("Name") => {
                let name = self
                    .statement
                    .and_then(|handle| statement::get(self.session, handle).ok())
                    .map(|statement| statement.name.clone())
                    .or_else(statement::running_name)
                    .unwrap_or_default();
                (V::Str(name), CastType::Char(Some(32)))
            }
            "stmt" if on("UpdateCount") => number(self.updated),
            "stmt" if on("TrigRecNo") => number(self.trigger.map_or(0, |t| t.recno)),
            "stmt" if on("TrigRowid") => match self.trigger {
                Some(t) => (V::Str(t.rowid.clone()), CastType::VarChar(Some(18))),
                None => null_text(18),
            },
            "stmt" if on("TrigName") => match self.trigger {
                Some(t) => (V::Str(t.name.clone()), CastType::VarChar(Some(200))),
                None => null_text(200),
            },
            "stmt" if on("TrigTableName") => match self.trigger {
                Some(t) => (V::Str(t.table.clone()), CastType::VarChar(Some(200))),
                None => null_text(200),
            },
            "stmt" if on("TrigEventType") => match self.trigger {
                Some(t) => number(t.event),
                None => null_number(),
            },
            "stmt" if on("TrigType") => match self.trigger {
                Some(t) => number(t.kind),
                None => null_number(),
            },
            "server" if on("OldestActiveTxn") => {
                match crate::transaction::oldest_active(self.session) {
                    Some((day, ms)) => (V::TimeStamp(day, ms), CastType::TimeStamp),
                    None => (V::Null, CastType::TimeStamp),
                }
            }
            _ => return None,
        })
    }
}
