//! A typical client session on the engine API: the same steps as
//! `crates/tflip-ace/examples/session.c`, without the ACE API in between.
//!
//! The session is written against [`Api`], so it runs on a [`Session`] in
//! the process or, with `--remote`, on a session of its own on a `tflipd`.
//! A remote data source is a path below one of the server's shares.
//!
//! ```text
//! cargo run -p tflip-protocol --example session -- [data source] [table]
//! cargo run -p tflip-protocol --example session -- \
//!     --remote HOST[:PORT] --fingerprint sha256:... [data source] [table]
//! ```
//!
//! Without `--fingerprint`, the fingerprint comes from
//! `TFLIP_SERVER_FINGERPRINT`.

use std::time::Duration;

use tflip_engine::Error;
use tflip_engine::connection::LOCAL;
use tflip_engine::format::FieldText;
use tflip_engine::getters::{FieldId, Scalar, Want};
use tflip_engine::handle::FIRST_HANDLE;
use tflip_engine::statement::{ParamRef, ParamValue};
use tflip_protocol::client::{Client, ConnectOptions};
use tflip_protocol::tls::{Trust, parse_fingerprint};
use tflip_protocol::{Api, DEFAULT_PORT, Handle, Session};

fn main() -> Result<(), String> {
    let mut remote = None;
    let mut fingerprint = std::env::var("TFLIP_SERVER_FINGERPRINT").ok();
    let mut positional = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--remote" => remote = Some(args.next().ok_or("--remote needs HOST[:PORT]")?),
            "--fingerprint" => {
                fingerprint = Some(args.next().ok_or("--fingerprint needs a value")?)
            }
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    let default_source = if remote.is_some() {
        r"fixtures\adt\dictionary"
    } else {
        "fixtures/adt/dictionary"
    };
    let source = positional
        .next()
        .unwrap_or_else(|| default_source.to_owned());
    let table = positional.next().unwrap_or_else(|| "customers".to_owned());

    let result = match remote {
        None => run(&Session::new(), &source, &table),
        Some(address) => {
            let (host, port) = match address.rsplit_once(':') {
                Some((host, port)) => (
                    host,
                    port.parse().map_err(|_| format!("bad port in {address}"))?,
                ),
                None => (address.as_str(), DEFAULT_PORT),
            };
            let fingerprint =
                fingerprint.ok_or("--remote needs --fingerprint or TFLIP_SERVER_FINGERPRINT")?;
            let fingerprint = parse_fingerprint(&fingerprint).ok_or("not a fingerprint")?;
            // The server hands out handles from `FIRST_HANDLE` on, as a local
            // session does; only a client with several engines (tflip-ace)
            // needs ranges that do not overlap.
            let name = "tflip-protocol session example";
            let options =
                ConnectOptions::new(Trust::pinned(vec![fingerprint]), Duration::from_secs(10));
            let client = Client::connect(host, port, FIRST_HANDLE, name, &options)
                .map_err(|e| format!("cannot connect to {address}: {}", e.reason))?;
            println!("connected to {}", client.server());
            run(&client, &source, &table)
        }
    };
    result.map_err(|e| format!("{e:?}"))
}

/// The session: settings, a connection to `source`, two queries, and the
/// teardown.
fn run(api: &impl Api, source: &str, table: &str) -> Result<(), Error> {
    // 1. Settings, for every connection of the session (`AdsShowDeleted`,
    // `AdsSetDateFormat`, `AdsSetEpoch`, `AdsSetExact`). A remote session
    // starts with the server's defaults, so they are sent to it like any
    // other call. `AdsSetDecimals` stays with the client and is not part of
    // the API.
    api.set_show_deleted(false)?;
    api.set_date_format(0, "YYYY-MM-DD".to_owned())?;
    api.set_epoch(1950)?;
    api.set_exact(0, false)?;

    // 2. Connect (`AdsConnect60`). The server is local to the session: the
    // process's, or the session tflipd keeps for this client.
    let connection = api.connect_path(source.to_owned(), LOCAL, None, None, None)?;

    // 3a. Run SQL directly (`AdsCreateSQLStatement`, `AdsExecuteSQLDirect`).
    let statement = api.create_statement(connection)?;
    let sql = format!("SELECT * FROM {table}");
    let cursor = api
        .execute_direct(statement, sql, true)?
        .ok_or(Error::NoCursorHandle)?;
    print_rows(api, cursor)?;
    api.close_table(cursor)?;

    // 3b. Or prepare it and set its parameters (`AdsPrepareSQL`, `AdsSetLong`,
    // `AdsExecuteSQL`); the statement is reused.
    api.prepare(
        statement,
        "SELECT :n * 2 AS twice FROM system.iota".to_owned(),
    )?;
    api.set_param(
        statement,
        ParamRef::Name("n".to_owned()),
        ParamValue::Integer(21),
    )?;
    let cursor = api.execute(statement, true)?.ok_or(Error::NoCursorHandle)?;
    print_rows(api, cursor)?;
    api.close_table(cursor)?;
    api.close_statement(statement)?;

    // 4. Disconnect, which closes whatever is still open on the connection.
    api.disconnect(connection)
}

/// Prints the rows of `cursor` from the top, a field at a time
/// (`AdsGotoTop`, `AdsAtEOF`, `AdsGetField`, `AdsSkip`).
fn print_rows(api: &impl Api, cursor: Handle) -> Result<(), Error> {
    let info = api.table_info(cursor)?;
    let names: Vec<String> = info
        .structure
        .fields
        .into_iter()
        .map(|field| field.name)
        .collect();
    println!("{}", names.join(" | "));
    api.goto_top(cursor)?;
    while !api.position(cursor)?.eof {
        let mut values = Vec::with_capacity(names.len());
        for name in &names {
            let field = FieldId::Name(name.clone());
            let want = Want::Field {
                utf8: true,
                ansi_dates: false,
            };
            values.push(match api.field_as(cursor, field, want) {
                Ok(Scalar::FieldText {
                    text: FieldText::Text(text),
                    ..
                }) => text.trim_end().to_owned(),
                Ok(Scalar::FieldText {
                    text: FieldText::Bytes(bytes),
                    ..
                }) => {
                    format!("<{} bytes>", bytes.len())
                }
                Ok(other) => format!("{other:?}"),
                Err(Error::InvalidFieldType) => "?".to_owned(),
                Err(e) => return Err(e),
            });
        }
        println!("{}", values.join(" | "));
        api.skip(cursor, 1)?;
    }
    Ok(())
}
