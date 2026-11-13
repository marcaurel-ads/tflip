//! Advantage SQL: statements parsed, their expressions typed and evaluated,
//! and `SELECT`s planned as live or static cursors.
//!
//! The engine runs statements: it opens the tables they name, evaluates
//! the expressions here on their records through [`Row`], and returns a
//! cursor, the table itself for a live `SELECT` and a table built in memory
//! for a static one. Nothing here knows about handles or files.
//!
//! What tflip runs is a subset of the original engine's SQL: `SELECT` with
//! expressions, joins, `WHERE`, grouping and aggregates, `UNION`,
//! subqueries, `ORDER BY`, `DISTINCT` and `TOP`; `INSERT`, `UPDATE` and
//! `DELETE` ([`dml`]), whose records the engine writes; `CREATE TABLE`,
//! `CREATE INDEX`, `DROP TABLE` and `DROP INDEX`, which it runs; `SHOW PLAN
//! FOR`; and `EXECUTE PROCEDURE`. Other DDL and scripts fail with error
//! 7200. The
//! language's types and messages are learnt from the original engine
//! (`fixtures/sql/`).
//!
//! A `SELECT` is bound to its tables ([`query`]) and lowered into a plan, a
//! tree of operators ([`ops`]), which the executor runs and `SHOW PLAN`
//! prints ([`explain`]): the plan shown is the plan run. Its scans look
//! their records up in the tables' indexes where the engine offers them
//! ([`query::Tables::indexes`]). A `SELECT` of one table's records that can
//! be a live cursor ([`mod@plan`]) is the table itself, filtered and sorted
//! as it is read: its `WHERE` as the AOF the original engine makes of it
//! ([`xbase`]), and its `ORDER BY` in an order whose key it is.

#![forbid(unsafe_code)]

pub mod ast;
pub mod compile;
pub mod dml;
pub mod error;
pub mod eval;
mod exec;
pub mod explain;
pub mod fts;
pub mod func;
mod lex;
mod like_order;
pub mod ops;
pub mod order_index;
pub mod params;
pub mod parse;
pub mod plan;
pub mod query;
pub mod random;
pub mod rowid;
pub mod script;
pub mod types;
pub mod value;
mod variable;
pub mod xbase;

pub use compile::{Compiler, FunctionSig, Node, Scope};
pub use error::SqlError;
pub use eval::{CaseFold, Env, Last, NoRow, Row, UserFunctions, eval, eval_filter};
pub use order_index::{KeyKind, Order, TempIndex, temp_index};
pub use parse::{Parsed, ViewRefusal, ViewSelect, check_view, check_view_property, parse};
pub use plan::{Column, Plan, SortKey, plan, plan_with, through_view, view_fields};
pub use random::Rand;
pub use types::Ty;
pub use value::{Blank, Collation, V};
