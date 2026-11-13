//! The SQL socket: runs statements and reads their cursors' rows.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tflip_engine::Error;
use tflip_engine::format::FieldText;
use tflip_engine::getters::{FieldId, Scalar, Want};
use tflip_protocol::{Api, Handle};
use ts_rs::TS;

use crate::connection::Connections;
use crate::error::ApiError;

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct Request {
    /// Echoed in the reply, so the frontend can match replies to requests.
    id: u32,
    #[serde(flatten)]
    command: Command,
}

#[derive(Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Command {
    Execute { sql: String },
    Rows { cursor: u32, start: u32, count: u32 },
    Close { cursor: u32 },
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct Reply {
    id: u32,
    #[serde(flatten)]
    answer: Answer,
}

#[derive(Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Answer {
    Cursor {
        cursor: u32,
        columns: Vec<Column>,
    },
    /// `rows`: none for a statement that counts none, such as DDL.
    Affected {
        rows: Option<u32>,
    },
    /// `end`: the cursor has no rows past these.
    Rows {
        rows: Vec<Vec<Option<String>>>,
        end: bool,
    },
    Closed,
    Failed(ApiError),
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct Column {
    name: String,
    /// The engine's name for the type, for display only.
    field_type: String,
}

#[derive(Clone, Copy)]
struct Cursor {
    statement: Handle,
    table: Handle,
}

#[derive(Default)]
struct Cursors {
    next: u32,
    open: HashMap<u32, Cursor>,
}

impl Cursors {
    fn add(&mut self, cursor: Cursor) -> u32 {
        let id = self.next;
        self.next += 1;
        self.open.insert(id, cursor);
        id
    }

    fn get(&self, id: u32) -> Result<Cursor, ApiError> {
        self.open
            .get(&id)
            .copied()
            .ok_or_else(|| Error::InvalidHandle.into())
    }

    fn remove(&mut self, id: u32) -> Result<Cursor, ApiError> {
        self.open
            .remove(&id)
            .ok_or_else(|| Error::InvalidHandle.into())
    }
}

/// Checks the connection before upgrading, so an unknown one gets an HTTP
/// error instead of a socket that fails on every command.
pub async fn open(
    State(connections): State<Arc<Connections>>,
    Path(id): Path<u32>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    connections.get(id)?;
    Ok(upgrade.on_upgrade(move |socket| serve(socket, connections, id)))
}

/// One request at a time, in order: a long statement holds up the paging
/// after it, which is fine until cancelling comes.
async fn serve(mut socket: WebSocket, connections: Arc<Connections>, id: u32) {
    let mut cursors = Cursors::default();
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            continue;
        };
        // Without an id there is nothing to reply to: a frontend bug.
        let Ok(request) = serde_json::from_str::<Request>(&text) else {
            break;
        };
        let answer = answer(&connections, id, &mut cursors, request.command)
            .await
            .unwrap_or_else(Answer::Failed);
        let reply = Reply {
            id: request.id,
            answer,
        };
        let reply = serde_json::to_string(&reply).expect("a reply serializes");
        if socket.send(Message::Text(reply.into())).await.is_err() {
            break;
        }
    }
    close_all(&connections, id, cursors).await;
}

async fn answer(
    connections: &Connections,
    id: u32,
    cursors: &mut Cursors,
    command: Command,
) -> Result<Answer, ApiError> {
    let connection = connections.get(id)?;
    match command {
        Command::Execute { sql } => {
            let executed = connection
                .call(move |api, handle| execute(api, handle, sql))
                .await?;
            Ok(match executed {
                Executed::Cursor { cursor, columns } => Answer::Cursor {
                    cursor: cursors.add(cursor),
                    columns,
                },
                Executed::Affected(rows) => Answer::Affected { rows },
            })
        }
        Command::Rows {
            cursor,
            start,
            count,
        } => {
            let cursor = cursors.get(cursor)?;
            let (rows, end) = connection
                .call(move |api, _| read_rows(api, cursor, start, count))
                .await?;
            Ok(Answer::Rows { rows, end })
        }
        Command::Close { cursor } => {
            let cursor = cursors.remove(cursor)?;
            connection
                .call(move |api, _| api.close_statement(cursor.statement))
                .await?;
            Ok(Answer::Closed)
        }
    }
}

/// Nobody is left to tell about a failure here; and after a Disconnect,
/// dropping the session already closed the statements.
async fn close_all(connections: &Connections, id: u32, cursors: Cursors) {
    let Ok(connection) = connections.get(id) else {
        return;
    };
    let statements: Vec<Handle> = cursors.open.into_values().map(|c| c.statement).collect();
    let _ = connection
        .call(move |api, _| {
            for statement in statements {
                api.close_statement(statement)?;
            }
            Ok(())
        })
        .await;
}

enum Executed {
    Cursor {
        cursor: Cursor,
        columns: Vec<Column>,
    },
    Affected(Option<u32>),
}

fn execute(api: &dyn Api, connection: Handle, sql: String) -> Result<Executed, Error> {
    let statement = api.create_statement(connection)?;
    let executed = execute_on(api, statement, sql);
    // A cursor keeps its statement open until the frontend closes it.
    if matches!(executed, Ok(Executed::Cursor { .. })) {
        return executed;
    }
    let closed = api.close_statement(statement);
    executed.and_then(|executed| closed.map(|()| executed))
}

fn execute_on(api: &dyn Api, statement: Handle, sql: String) -> Result<Executed, Error> {
    let Some(table) = api.execute_direct(statement, sql, true)? else {
        let rows = api.affected(statement)?;
        // AdsGetRecordCount's value for a statement that counts no rows.
        return Ok(Executed::Affected((rows != u32::MAX).then_some(rows)));
    };
    let columns = api
        .table_info(table)?
        .structure
        .fields
        .into_iter()
        .map(|field| Column {
            name: field.name,
            field_type: format!("{:?}", field.field_type),
        })
        .collect();
    Ok(Executed::Cursor {
        cursor: Cursor { statement, table },
        columns,
    })
}

/// `AdsGetField`'s text: the only getter that takes every type.
const WANT: Want = Want::Field {
    utf8: false,
    ansi_dates: false,
};

/// A row as the grid shows it: `None` for NULL.
type Row = Vec<Option<String>>;

/// From the top each time: simple, and fast enough for the pages a person
/// scrolls through.
fn read_rows(
    api: &dyn Api,
    cursor: Cursor,
    start: u32,
    count: u32,
) -> Result<(Vec<Row>, bool), Error> {
    api.goto_top(cursor.table)?;
    if start > 0 {
        api.skip(cursor.table, i64::from(start))?;
    }
    let mut rows = Vec::new();
    while rows.len() < count as usize && !api.position(cursor.table)?.eof {
        rows.push(read_row(api, cursor.table)?);
        api.skip(cursor.table, 1)?;
    }
    Ok((rows, api.position(cursor.table)?.eof))
}

fn read_row(api: &dyn Api, table: Handle) -> Result<Row, Error> {
    let values = api.read_fields(table, WANT)?;
    let mut row = Vec::with_capacity(values.len());
    for (index, value) in values.into_iter().enumerate() {
        let field = index + 1;
        // NULL reads as empty text, which the grid must tell apart.
        if api.is_null(table, field)? {
            row.push(None);
            continue;
        }
        // A whole-row read leaves memos out.
        let value = value.unwrap_or_else(|| api.field_as(table, FieldId::Number(field), WANT));
        row.push(Some(cell(value)));
    }
    Ok(row)
}

fn cell(value: Result<Scalar, Error>) -> String {
    match value {
        Ok(Scalar::FieldText {
            text: FieldText::Text(text),
            ..
        }) => text,
        Ok(Scalar::FieldText {
            text: FieldText::Bytes(bytes),
            ..
        }) => {
            format!("({} bytes)", bytes.len())
        }
        Ok(_) => unreachable!("Want::Field gives field text"),
        // A binary memo has no text; one such cell must not fail the page.
        Err(error) => format!("({})", error.to_ace().message()),
    }
}

#[cfg(test)]
mod tests {
    use tflip_engine::connection;
    use tflip_protocol::Session;

    use super::*;

    fn scratch(test: &str) -> String {
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-studio-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        tflip_storage::universal_path(dir.to_str().unwrap())
    }

    #[test]
    fn executes_and_pages_through_a_cursor() {
        let session = Session::new();
        let connection = session
            .connect_path(scratch("pages"), connection::LOCAL, None, None, None)
            .unwrap();
        let run = |sql: &str| execute(&session, connection, sql.into()).unwrap();

        assert!(matches!(
            run("CREATE TABLE t (n Integer, s Char(3), m Memo)"),
            Executed::Affected(None)
        ));
        assert!(matches!(
            run("INSERT INTO t VALUES (1, 'a', 'long')"),
            Executed::Affected(Some(1))
        ));
        run("INSERT INTO t VALUES (2, NULL, NULL)");

        let Executed::Cursor { cursor, columns } = run("SELECT * FROM t ORDER BY n") else {
            panic!("no cursor");
        };
        let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["n", "s", "m"]);

        let (rows, end) = read_rows(&session, cursor, 0, 1).unwrap();
        assert_eq!(
            rows,
            [[Some("1".into()), Some("a  ".into()), Some("long".into())]]
        );
        assert!(!end);
        let (rows, end) = read_rows(&session, cursor, 1, 10).unwrap();
        assert_eq!(rows, [[Some("2".into()), None, None]]);
        assert!(end);
    }

    #[test]
    fn closes_the_statement_of_failed_sql() {
        let session = Session::new();
        let connection = session
            .connect_path(scratch("fails"), connection::LOCAL, None, None, None)
            .unwrap();
        assert!(execute(&session, connection, "SELEKT".into()).is_err());
    }
}
