//! Replacement for the Advantage Client Engine DLL: Cargo builds it as
//! `tflip_ace.dll`, which `cargo xtask dist` installs as `ace32.dll` (32-bit)
//! and `ace64.dll` (64-bit), the names clients load.
//!
//! Every export uses the ACE calling convention (`WINAPI`, i.e. `stdcall` on
//! 32-bit Windows) and returns an ACE error code. On `i686-pc-windows-gnu` the
//! names must be exported undecorated (`AdsGetVersion`, not
//! `AdsGetVersion@20`), which `.cargo/config.toml` arranges.
//!
//! Exports that only pretend to work call `stub!`, which reports their
//! first call (see `src/log.rs`); `grep -rn 'stub!(' crates/tflip-ace` lists
//! them.
//!
//! This crate is a thin layer over [`tflip_engine`]: the exports check and
//! convert pointers and ANSI strings, call the engine, and turn its results
//! into ACE error codes. State and logic live in the engine.
//!
//! The unsafe code is the C ABI's: an export reads its pointer arguments
//! into Rust values, leaves the work to safe code, and writes the results
//! back to the client's pointers. The readers and writers the exports share
//! are in `ffi.rs`; each unsafe block says which part of the ACE contract
//! (the export's `# Safety` section) it relies on.
//!
//! The exports are grouped by what they act on, and all of them are also
//! re-exported at the crate root:
//!
//! - the process and its connections: [`settings`], [`connection`],
//!   [`handle`], [`mgmt`] (the management functions), [`dictionary`] (data
//!   dictionaries) and the last error (`last_error.rs`);
//! - tables: [`table`], [`ddl`] (creating and maintaining tables and
//!   orders), [`record`] (moving, reading and writing records, locks),
//!   [`field`] (what fields are), [`getters`] and [`setters`] (their
//!   values, and the parameters of statements), [`filter`] (AOFs and record
//!   filters) and [`expr`] (expressions);
//! - index orders: [`index`] (the orders), [`keys`] (seeking and building
//!   keys) and [`scope`] (scopes and positions in an order);
//! - SQL: [`statement`];
//! - [`convert`] (dates and times as text), and the features tflip lacks:
//!   [`transaction`], [`encryption`] and [`unimplemented`](mod@unimplemented) (the rest).

#![allow(non_snake_case)]
#![warn(clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]
// The tests call the exports as a client does, with pointers to their own
// values; the library's build holds the crate's own code to the lint.
#![cfg_attr(test, allow(clippy::undocumented_unsafe_blocks))]

#[cfg(test)]
mod aes_api;
mod ansi;
mod backend;
#[cfg(test)]
mod backup;
mod clipboard;
mod codepages;
mod config;
mod connect_options;
mod dbcs;
#[cfg(test)]
mod differential;
#[cfg(test)]
mod events;
mod ffi;
mod find_servers;
#[cfg(test)]
pub(crate) mod fixtures;
mod last_error;
mod log;
#[cfg(test)]
mod mismatch;
#[cfg(test)]
mod open_cache;
mod pieces;
mod remote;
#[cfg(test)]
mod round_trips;
#[cfg(test)]
mod writes;

pub mod bookmark;
pub mod connection;
pub mod convert;
pub mod ddl;
pub mod dictionary;
pub mod encryption;
pub mod expr;
pub mod field;
pub mod filter;
pub mod free_tables;
pub mod getters;
pub mod handle;
pub mod index;
pub mod keys;
pub mod mgmt;
pub mod param_n;
pub mod param_n_mg;
pub mod record;
pub mod relation;
pub mod scope;
pub mod setters;
pub mod settings;
pub mod statement;
pub mod table;
pub mod transaction;
pub mod unimplemented;

pub use bookmark::*;
pub use connection::*;
pub use convert::*;
pub use ddl::*;
pub use dictionary::*;
pub use encryption::*;
pub use expr::*;
pub use field::*;
pub use filter::*;
pub use getters::*;
pub use handle::*;
pub use index::*;
pub use keys::*;
pub use last_error::AdsGetLastError;
pub use mgmt::*;
pub use record::*;
pub use relation::*;
pub use scope::*;
pub use setters::*;
pub use settings::*;
pub use statement::*;
pub use table::*;
pub use transaction::*;

use last_error::fail;

pub(crate) use ffi::{
    wide_string, write_cut, write_cut_at, write_name, write_string, write_string_at,
};

use std::sync::LazyLock;

use tflip_engine::Session;

/// The engine session of the process: the local server's connections,
/// tables and settings, which ACE keeps process-wide.
pub(crate) static LOCAL: LazyLock<Session> = LazyLock::new(|| {
    let session = Session::new();
    tflip_engine::settings::set_real_transactions(&session, config::real_transactions());
    // The tests turn the local caches on where they want them.
    if !cfg!(test) && config::local_caching() {
        tflip_engine::settings::set_local_caching(&session, true);
    }
    if let Some(log) = config::transaction_log() {
        tflip_engine::txlog::set_folder(log);
    }
    // The tests' transactions log to a folder of their own, unless they are
    // told one (a process of a test's that is to crash).
    #[cfg(test)]
    if std::env::var_os("TFLIP_TX_LOG").is_none() {
        let folder = std::env::temp_dir().join("tflip-ace-tx");
        tflip_engine::txlog::set_folder(folder);
    }
    tflip_engine::settings::set_collations(&session, local_collations());
    tflip_engine::settings::set_oem_text(ansi::ansi_to_oem, ansi::oem_to_ansi_text);
    tflip_engine::aep::set_libraries(Box::new(tflip_aep::Loader::local(|call| {
        last_error::outside(call)
    })));
    session
});

/// The collations of the local server: those of `adslocal.cfg` in the
/// collation folder. With ANSI_CHAR_SET empty, the static collation is the
/// one the host's locale derives, as in the original engine (the tests' is
/// the default, whatever this machine's locale is).
pub(crate) fn local_collations() -> tflip_engine::collations::Collations {
    let system = if cfg!(test) {
        None
    } else {
        tflip_engine::system_collation::host()
    };
    match config::collation_dir() {
        Some(dir) => tflip_engine::collations::Collations::from_folder_for(&dir, system),
        None => tflip_engine::collations::Collations::system(system),
    }
}

pub type Unsigned8 = u8;
pub type Unsigned16 = u16;
pub type Unsigned32 = u32;
pub type Signed32 = i32;
/// A UTF-16 code unit.
pub type Wchar = u16;
/// `unsigned long` on 32-bit Windows, `unsigned __int64` on x64.
pub type AdsHandle = usize;

pub use tflip_engine::ace_error::codes::*;

pub const ADS_FALSE: Unsigned16 = 0;
pub const ADS_TRUE: Unsigned16 = 1;

pub const ADS_DEFAULT: Unsigned16 = 0;

pub const ADS_RESPECTFILTERS: Unsigned16 = 0x0001;
pub const ADS_IGNOREFILTERS: Unsigned16 = 0x0002;
pub const ADS_RESPECTSCOPES: Unsigned16 = 0x0003;
pub const ADS_REFRESHCOUNT: Unsigned16 = 0x0004;

pub const ADS_TOP: Unsigned16 = 1;
pub const ADS_BOTTOM: Unsigned16 = 2;

/// Field and key types (those `AdsGetKeyType` reports).
pub const ADS_LOGICAL: Unsigned16 = 1;
pub const ADS_NUMERIC: Unsigned16 = 2;
pub const ADS_DATE: Unsigned16 = 3;
pub const ADS_STRING: Unsigned16 = 4;
pub const ADS_BINARY: Unsigned16 = 6;
pub const ADS_IMAGE: Unsigned16 = 7;
pub const ADS_RAW: Unsigned16 = 16;

pub const ADS_SOFTSEEK: Unsigned16 = 0x0001;
pub const ADS_HARDSEEK: Unsigned16 = 0x0002;
pub const ADS_SEEKGT: Unsigned16 = 0x0004;

pub const ADS_RAWKEY: Unsigned16 = 1;
pub const ADS_STRINGKEY: Unsigned16 = 2;
pub const ADS_DOUBLEKEY: Unsigned16 = 4;
pub const ADS_WSTRINGKEY: Unsigned16 = 8;

pub const ADS_NONE: Unsigned16 = 0x0000;
pub const ADS_LTRIM: Unsigned16 = 0x0001;
pub const ADS_RTRIM: Unsigned16 = 0x0002;
pub const ADS_TRIM: Unsigned16 = 0x0003;
pub const ADS_GET_UTF8: Unsigned16 = 0x0004;
pub const ADS_DONT_CHECK_CONV_ERR: Unsigned16 = 0x0008;
pub const ADS_GET_FORMAT_ANSI: Unsigned16 = 0x0010;
pub const ADS_GET_GUID_MIME: Unsigned16 = 0x0100;
pub const ADS_GET_GUID_FILE: Unsigned16 = 0x0200;
pub const ADS_GET_GUID_NUMBERS: Unsigned16 = 0x0400;
pub const ADS_GET_GUID_REGISTRY: Unsigned16 = 0x0800;

pub const ADS_CODEUNIT_LENGTH: Unsigned32 = 0;
pub const ADS_BYTE_LENGTH: Unsigned32 = 0x0000_0001;
pub const ADS_BYTE_LENGTH_IN_BUFFER: Unsigned32 = 0x0000_0002;

pub const ADS_ANSI: Unsigned16 = 1;
pub const ADS_OEM: Unsigned16 = 2;

pub const ADS_EXCLUSIVE: Unsigned32 = 0x0000_0001;
pub const ADS_READONLY: Unsigned32 = 0x0000_0002;
pub const ADS_SHARED: Unsigned32 = 0x0000_0004;
pub const ADS_CLIPPER_MEMOS: Unsigned32 = 0x0000_0008;
pub const ADS_TABLE_PERM_READ: Unsigned32 = 0x0000_0010;
pub const ADS_TABLE_PERM_UPDATE: Unsigned32 = 0x0000_0020;
pub const ADS_TABLE_PERM_INSERT: Unsigned32 = 0x0000_0040;
pub const ADS_TABLE_PERM_DELETE: Unsigned32 = 0x0000_0080;
pub const ADS_FREE_TABLE: Unsigned32 = 0x0000_1000;
pub const ADS_TEMP_TABLE: Unsigned32 = 0x0000_2000;

pub const ADS_DATABASE_TABLE: Unsigned16 = ADS_DEFAULT;
pub const ADS_NTX: Unsigned16 = 1;
pub const ADS_CDX: Unsigned16 = 2;
pub const ADS_ADT: Unsigned16 = 3;
pub const ADS_VFP: Unsigned16 = 4;

pub const ADS_LOCAL_SERVER: Unsigned16 = 0x0001;
pub const ADS_REMOTE_SERVER: Unsigned16 = 0x0002;
pub const ADS_AIS_SERVER: Unsigned16 = 0x0004;

pub const ADS_COMPATIBLE_LOCKING: Unsigned16 = 0;
pub const ADS_PROPRIETARY_LOCKING: Unsigned16 = 1;

pub const ADS_CHECKRIGHTS: Unsigned16 = 1;
pub const ADS_IGNORERIGHTS: Unsigned16 = 2;

pub const ADS_CONNECTION: Unsigned16 = 1;
pub const ADS_TABLE: Unsigned16 = 2;
pub const ADS_INDEX_ORDER: Unsigned16 = 3;
pub const ADS_STATEMENT: Unsigned16 = 4;
pub const ADS_CURSOR: Unsigned16 = 5;
pub const ADS_DATABASE_CONNECTION: Unsigned16 = 6;
pub const ADS_FTS_INDEX_ORDER: Unsigned16 = 8;

pub const ADS_CURSOR_READONLY: Unsigned16 = 1;
pub const ADS_CURSOR_READWRITE: Unsigned16 = 2;

pub const ADS_CONSTRAIN: Unsigned16 = 1;
pub const ADS_NO_CONSTRAIN: Unsigned16 = 2;

pub const ADS_READ_ALL_COLUMNS: Unsigned16 = 1;
pub const ADS_READ_SELECT_COLUMNS: Unsigned16 = 2;

pub const ADS_MIN_ADI_PAGESIZE: Unsigned32 = 512;
pub const ADS_MAX_ADI_PAGESIZE: Unsigned32 = 8192;

/// `error`'s code for a call that sets a filter: as [`error_code`], with an
/// expression that does not compile reported with the codes of record
/// filters.
pub(crate) fn filter_error_code(error: tflip_engine::Error) -> Unsigned32 {
    match error {
        tflip_engine::Error::Expression(kind) => fail(kind.code(FILTER_ERRORS)),
        error => error_code(error),
    }
}

/// The ACE error code for an engine error, recorded as the thread's last
/// error (a handle error as a failed handle check; see [`last_error`]).
pub(crate) fn error_code(error: tflip_engine::Error) -> Unsigned32 {
    use tflip_engine::ace_error::Text;
    let error = error.to_ace();
    match (error.text, error.handle_check) {
        (Text::Standard, false) => fail(error.code),
        (Text::Standard, true) => last_error::fail_handle(error.code),
        (Text::Detail(detail), false) => last_error::fail_detail(error.code, &detail),
        (Text::Detail(detail), true) => last_error::fail_handle_detail(error.code, &detail),
        (Text::Own(text), _) => last_error::fail_text(error.code, text),
    }
}

/// The ACE code for the result of an engine call.
pub(crate) fn code(result: Result<(), tflip_engine::Error>) -> Unsigned32 {
    match result {
        Ok(()) => AE_SUCCESS,
        Err(error) => error_code(error),
    }
}
