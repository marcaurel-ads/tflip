//! Online backup and restore (`sp_BackupDatabase`, `sp_RestoreDatabase`,
//! `sp_BackupFreeTables`, `sp_RestoreFreeTables`), as the original's remote
//! server does them (`fixtures/backup/`).
//!
//! A backup writes an image: a folder with copies of the files, each table's
//! made by [`tflip_storage::image`]; a restore reads one. The procedures'
//! argument checks are in [`crate::procedure`]; the options, the rows of the
//! result set and the work of a dictionary's backup and restore are here.

pub(crate) mod active;
pub(crate) mod database;
pub(crate) mod differential;
pub(crate) mod free;
pub(crate) mod options;
pub(crate) mod paths;
pub(crate) mod rows;
