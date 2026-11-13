//! The connection repository: the connections a user made before, in an ADT
//! table of their own, so the studio keeps its data the way it edits others'.

use std::path::Path;

use anyhow::Context;
use serde::Serialize;
use tflip_engine::Error;
use tflip_engine::connection;
use tflip_engine::format::FieldText;
use tflip_engine::getters::{FieldId, Scalar, Want};
use tflip_engine::statement::{ParamRef, ParamValue};
use tflip_protocol::{Api, Handle, Session};
use ts_rs::TS;

use crate::vault::{Cost, Master};

const TABLE: &str = "connections";

/// A GUID rather than an AutoInc: whoever adds an entry can make its ID,
/// without asking the table for it afterwards.
///
/// The layout holds the stored password from the start, so adding it later
/// needs no restructure of tables already out there.
const CREATE: &str = "CREATE TABLE connections (
    ID GUID,
    Name NVarChar(100),
    Path NVarChar(1024),
    UserName NVarChar(128),
    Password NVarChar(256),
    LastUsed TimeStamp)";

/// One row at most: what unlocks the stored passwords (see [`Master`]).
const CREATE_MASTER: &str = "CREATE TABLE master (
    Salt NVarChar(64),
    Proof NVarChar(128),
    KdfMemory Integer,
    KdfPasses Integer,
    KdfLanes Integer)";

const COLUMNS: &str = "ID, Name, Path, UserName, Password";

#[derive(Serialize, TS)]
#[ts(export)]
pub struct RepositoryEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub user: Option<String>,
    /// Whether its password is stored.
    pub saved: bool,
}

/// An entry as the repository holds it, with its sealed password.
pub struct Stored {
    pub entry: RepositoryEntry,
    pub sealed: Option<String>,
}

pub struct Repository {
    folder: String,
}

impl Repository {
    pub fn open(folder: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("cannot create {}", folder.display()))?;
        let repository = Repository {
            folder: folder
                .to_str()
                .context("the folder is not UTF-8")?
                .to_owned(),
        };
        // Each on its own: a repository from before the master password has
        // its connections but no master yet.
        for (table, create) in [(TABLE, CREATE), ("master", CREATE_MASTER)] {
            if !folder.join(format!("{table}.adt")).exists() {
                repository
                    .run(create, &[])
                    .map_err(|error| anyhow::anyhow!(error.to_ace().message()))
                    .with_context(|| {
                        format!("cannot create the repository in {}", folder.display())
                    })?;
            }
        }
        Ok(repository)
    }

    /// The last used first, as the studio reopens it.
    pub fn list(&self) -> Result<Vec<Stored>, Error> {
        let sql = format!("SELECT {COLUMNS} FROM connections ORDER BY LastUsed DESC");
        Ok(self.query(&sql, &[])?.into_iter().map(stored).collect())
    }

    pub fn get(&self, id: &str) -> Result<Stored, Error> {
        let sql = format!("SELECT {COLUMNS} FROM connections WHERE ID = :id");
        let mut rows = self.query(&sql, &[("id", ParamValue::Text(id.to_owned()))])?;
        rows.pop().map(stored).ok_or(Error::InvalidHandle)
    }

    /// After a connect succeeded, so a mistyped path never lands here.
    /// `sealed` replaces the stored password; `None` keeps it.
    pub fn remember(
        &self,
        path: &str,
        user: Option<&str>,
        sealed: Option<&str>,
    ) -> Result<(), Error> {
        let params = [
            ("path", ParamValue::Text(path.to_owned())),
            // '' rather than NULL, so `=` finds an entry without a user.
            (
                "user",
                ParamValue::Text(user.unwrap_or_default().to_owned()),
            ),
            (
                "password",
                ParamValue::Text(sealed.unwrap_or_default().to_owned()),
            ),
        ];
        let update = match sealed {
            Some(_) => {
                "UPDATE connections SET LastUsed = NOW(), Password = :password
                        WHERE Path = :path AND UserName = :user"
            }
            None => {
                "UPDATE connections SET LastUsed = NOW()
                     WHERE Path = :path AND UserName = :user"
            }
        };
        let updated = self.run(update, &params[..if sealed.is_some() { 3 } else { 2 }])?;
        if updated == 0 {
            let name = ParamValue::Text(name_of(path));
            self.run(
                "INSERT INTO connections (ID, Name, Path, UserName, Password, LastUsed)
                 VALUES (NEWID(), :name, :path, :user, :password, NOW())",
                &[
                    params[0].clone(),
                    params[1].clone(),
                    params[2].clone(),
                    ("name", name),
                ],
            )?;
        }
        Ok(())
    }

    pub fn master(&self) -> Result<Option<Master>, Error> {
        let rows = self.query(
            "SELECT Salt, Proof, KdfMemory, KdfPasses, KdfLanes FROM master",
            &[],
        )?;
        let number = |text: &str| text.trim().parse().map_err(|_| Error::InvalidData);
        rows.into_iter()
            .next()
            .map(|row| {
                Ok(Master {
                    salt: row[0].clone(),
                    proof: row[1].clone(),
                    cost: Cost {
                        memory: number(&row[2])?,
                        passes: number(&row[3])?,
                        lanes: number(&row[4])?,
                    },
                })
            })
            .transpose()
    }

    /// Only for a repository without one: a new master could not open the
    /// passwords sealed under the old.
    pub fn set_master(&self, master: &Master) -> Result<(), Error> {
        let number = |n: u32| ParamValue::Integer(n.into());
        self.run(
            "INSERT INTO master VALUES (:salt, :proof, :memory, :passes, :lanes)",
            &[
                ("salt", ParamValue::Text(master.salt.clone())),
                ("proof", ParamValue::Text(master.proof.clone())),
                ("memory", number(master.cost.memory)),
                ("passes", number(master.cost.passes)),
                ("lanes", number(master.cost.lanes)),
            ],
        )?;
        Ok(())
    }

    pub fn forget(&self, id: &str) -> Result<(), Error> {
        let deleted = self.run(
            "DELETE FROM connections WHERE ID = :id",
            &[("id", ParamValue::Text(id.to_owned()))],
        )?;
        if deleted == 0 {
            return Err(Error::InvalidHandle);
        }
        Ok(())
    }

    /// A session per call, closed when it returns: the table stays shared
    /// with a second studio, and nothing is left open between calls.
    fn with<R>(&self, f: impl FnOnce(&dyn Api, Handle) -> Result<R, Error>) -> Result<R, Error> {
        let session = Session::new();
        let connection =
            session.connect_path(self.folder.clone(), connection::LOCAL, None, None, None)?;
        let statement = session.create_statement(connection)?;
        f(&session, statement)
    }

    fn prepare(
        api: &dyn Api,
        statement: Handle,
        sql: &str,
        params: &[(&str, ParamValue)],
    ) -> Result<(), Error> {
        api.prepare(statement, sql.to_owned())?;
        for (name, value) in params {
            api.set_param(statement, ParamRef::Name((*name).to_owned()), value.clone())?;
        }
        Ok(())
    }

    fn run(&self, sql: &str, params: &[(&str, ParamValue)]) -> Result<u32, Error> {
        self.with(|api, statement| {
            Self::prepare(api, statement, sql, params)?;
            api.execute(statement, false)?;
            api.affected(statement)
        })
    }

    /// Every field as text, which is all the repository holds or parses.
    fn query(&self, sql: &str, params: &[(&str, ParamValue)]) -> Result<Vec<Vec<String>>, Error> {
        self.with(|api, statement| {
            Self::prepare(api, statement, sql, params)?;
            let table = api
                .execute(statement, true)?
                .expect("a SELECT gives a cursor");
            let fields = api.table_info(table)?.structure.fields.len();
            let mut rows = Vec::new();
            api.goto_top(table)?;
            while !api.position(table)?.eof {
                let row = (1..=fields)
                    .map(|field| text(api, table, field))
                    .collect::<Result<_, _>>()?;
                rows.push(row);
                api.skip(table, 1)?;
            }
            Ok(rows)
        })
    }
}

/// From a row of [`COLUMNS`]; empty text is no user and no password, as
/// `remember` writes them.
fn stored(mut row: Vec<String>) -> Stored {
    let some = |text: String| (!text.is_empty()).then_some(text);
    let sealed = some(row.pop().unwrap());
    let user = some(row.pop().unwrap());
    let path = row.pop().unwrap();
    let name = row.pop().unwrap();
    let id = row.pop().unwrap();
    Stored {
        entry: RepositoryEntry {
            id,
            name,
            path,
            user,
            saved: sealed.is_some(),
        },
        sealed,
    }
}

/// The folder's or dictionary's own name, as a first guess the user can
/// change.
fn name_of(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    if name.is_empty() { path } else { name }.to_owned()
}

fn text(api: &dyn Api, table: Handle, field: usize) -> Result<String, Error> {
    let want = Want::Field {
        utf8: true,
        ansi_dates: false,
    };
    match api.field_as(table, FieldId::Number(field), want)? {
        Scalar::FieldText {
            text: FieldText::Text(text),
            ..
        } => Ok(text),
        _ => unreachable!("Want::Field gives field text"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(test: &str) -> std::path::PathBuf {
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-studio-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn remembers_each_connection_once_and_the_last_used_first() {
        let repository = Repository::open(&scratch("remembers")).unwrap();
        // A TimeStamp counts milliseconds, which three calls in a row share.
        let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
        repository.remember("/data/sales", None, None).unwrap();
        pause();
        repository
            .remember("/data/äöü/dict.add", Some("ADSSYS"), None)
            .unwrap();
        pause();
        repository.remember("/data/sales", None, None).unwrap();

        let entries = repository.list().unwrap();
        let shown: Vec<_> = entries
            .iter()
            .map(|s| {
                let e = &s.entry;
                (e.name.as_str(), e.path.as_str(), e.user.as_deref(), e.saved)
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("sales", "/data/sales", None, false),
                ("dict.add", "/data/äöü/dict.add", Some("ADSSYS"), false),
            ]
        );
        let id = &entries[1].entry.id;
        assert_eq!(repository.get(id).unwrap().entry.path, "/data/äöü/dict.add");
    }

    #[test]
    fn keeps_a_sealed_password_until_another_replaces_it() {
        let repository = Repository::open(&scratch("sealed")).unwrap();
        let remember = |sealed| repository.remember("/d.add", Some("U"), sealed).unwrap();
        let sealed = || repository.list().unwrap().pop().unwrap().sealed;
        remember(Some("first"));
        assert_eq!(sealed().as_deref(), Some("first"));
        remember(None);
        assert_eq!(sealed().as_deref(), Some("first"));
        remember(Some("second"));
        assert_eq!(sealed().as_deref(), Some("second"));
        assert!(repository.list().unwrap()[0].entry.saved);
    }

    #[test]
    fn keeps_the_master() {
        let folder = scratch("master");
        let repository = Repository::open(&folder).unwrap();
        assert_eq!(repository.master().unwrap(), None);
        let master = Master {
            salt: "salt".into(),
            proof: "proof".into(),
            cost: crate::vault::COST,
        };
        repository.set_master(&master).unwrap();
        let repository = Repository::open(&folder).unwrap();
        assert_eq!(repository.master().unwrap(), Some(master));
    }

    #[test]
    fn forgets_an_entry() {
        let folder = scratch("forgets");
        let repository = Repository::open(&folder).unwrap();
        repository.remember("/data/sales", None, None).unwrap();
        let id = repository.list().unwrap()[0].entry.id.clone();
        repository.forget(&id).unwrap();
        // Opened again: the table is kept, not made anew.
        let repository = Repository::open(&folder).unwrap();
        assert!(repository.list().unwrap().is_empty());
        assert!(matches!(repository.get(&id), Err(Error::InvalidHandle)));
        assert!(matches!(repository.forget(&id), Err(Error::InvalidHandle)));
    }
}
