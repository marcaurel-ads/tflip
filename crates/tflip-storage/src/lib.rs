//! Storage engine for Advantage-compatible files (`.adt`, `.adm`, `.adi`,
//! `.dbf`, ...).
//!
//! The fixtures in `fixtures/` are the ground truth the readers and writers
//! are tested against.

#![deny(unsafe_code)]

pub mod adi;
mod adm;
pub mod adt;
pub mod archive;
pub mod build;
mod bytes;
pub mod cdx;
pub mod collation;
pub mod cp1252;
pub mod create;
pub mod crypto;
pub mod dbf;
pub mod dd;
pub mod field;
pub mod icu;
pub mod image;
pub mod key;
mod lock;
pub use lock::try_lock_exclusive;
pub use os::path::universal_path;
pub mod maintain;
pub mod memo;
pub mod ntx;
mod ntx_write;
pub mod order;
#[allow(unsafe_code)]
mod os;
pub mod process;
pub mod source;
pub mod table;
#[doc(hidden)]
pub mod testing;
pub mod undefined;
pub mod value;
mod xmemo;
