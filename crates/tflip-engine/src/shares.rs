//! Shares: the folders a server's sessions may reach.
//!
//! A session in the client library reaches any path. A server's sessions
//! reach only its shares: a connection names its data by share
//! (`data\sales` is the folder `sales` in the share `data`), and table names
//! and table listings stay inside the connection's folder. Anything else is
//! [`Error::FileNotOnServer`]. Names are looked up regardless of case, as
//! clients name files the way Windows lets them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::Error;

/// A server's shares, by name.
#[derive(Debug, Clone, Default)]
pub struct Shares {
    /// The folders, by lower-case name.
    folders: BTreeMap<String, PathBuf>,
    /// Absolute paths as clients name them (`c:\data`, lower case, `\`
    /// between parts) and the share each stands for, longest first.
    maps: Vec<(String, String)>,
    check_free_table_access: bool,
    disable_free_connections: bool,
}

impl Shares {
    /// No shares yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the share `name` for `folder`; names match regardless of case.
    pub fn add(&mut self, name: &str, folder: impl Into<PathBuf>) {
        self.folders.insert(name.to_lowercase(), folder.into());
    }

    /// Maps the absolute path `prefix` as clients name it (`C:\data`) to the
    /// share `share`: a path under it is the same path inside the share, so
    /// that clients that name their tables by the original server's paths
    /// reach them (regardless of case and of `\` or `/`).
    pub fn add_map(&mut self, prefix: &str, share: &str) {
        let prefix = normal(prefix).trim_end_matches('\\').to_owned();
        self.maps.retain(|(known, _)| *known != prefix);
        self.maps.push((prefix, share.to_owned()));
        self.maps
            .sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    }

    /// The path inside a share that the absolute path `name` stands for by
    /// [`Shares::add_map`], if a map takes it in.
    pub(crate) fn mapped(&self, name: &str) -> Option<Result<PathBuf, Error>> {
        let normal = normal(name);
        self.maps.iter().find_map(|(prefix, share)| {
            let rest = normal.strip_prefix(prefix.as_str())?;
            if !(rest.is_empty() || rest.starts_with('\\')) {
                return None;
            }
            // The rest as the client spelled it.
            let rest = &name[name.len() - rest.len()..];
            Some(match self.folder(share) {
                Some(folder) => descend(folder.to_path_buf(), parts(rest)),
                None => Err(Error::FileNotOnServer),
            })
        })
    }

    /// `name`, a path inside `base` as a client names it (a table, or the
    /// folder of a table listing). An absolute path is
    /// [`Error::FileNotOnServer`], but for one a map takes in
    /// ([`Shares::add_map`]).
    pub(crate) fn inside(&self, base: &Path, name: &str) -> Result<PathBuf, Error> {
        match self.mapped(name) {
            Some(path) => path,
            None => inside(base, name),
        }
    }

    /// Sets the server setting `Check_Free_Table_Access`: no connection to a
    /// dictionary opens a free table, the administrators' included.
    pub fn set_check_free_table_access(&mut self, on: bool) {
        self.check_free_table_access = on;
    }

    /// Sets the server setting `Disable_Free_Connections`: every connection
    /// must be to a dictionary.
    pub fn set_disable_free_connections(&mut self, on: bool) {
        self.disable_free_connections = on;
    }

    pub(crate) fn check_free_table_access(&self) -> bool {
        self.check_free_table_access
    }

    pub(crate) fn disable_free_connections(&self) -> bool {
        self.disable_free_connections
    }

    /// The folder of a share, if there is one by that name.
    pub fn folder(&self, name: &str) -> Option<&Path> {
        self.folders.get(&name.to_lowercase()).map(PathBuf::as_path)
    }

    /// Whether `path` lies in one of the shares, `.` and `..` parts (of either)
    /// worked out.
    pub(crate) fn contains(&self, path: &Path) -> bool {
        let path = crate::link::resolve(Path::new("/"), &path.to_string_lossy());
        self.folders.values().any(|folder| {
            path.starts_with(crate::link::resolve(
                Path::new("/"),
                &folder.to_string_lossy(),
            ))
        })
    }

    /// The folder or file a connection path names: its first part is the
    /// share, the rest a path inside it.
    pub(crate) fn connection_path(&self, path: &str) -> Result<PathBuf, Error> {
        if let Some(path) = self.mapped(path) {
            return path;
        }
        let mut parts = parts(path);
        let share = parts.next().ok_or(Error::FileNotOnServer)?;
        let folder = self.folder(share).ok_or(Error::FileNotOnServer)?;
        descend(folder.to_path_buf(), parts)
    }
}

/// `path` in lower case with `\` between its parts.
fn normal(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// The parts of a relative path as clients write it, `\` or `/` between
/// them; empty parts and `.` are left out.
fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
}

/// `base` with `parts` added, each found regardless of case. `..`, and a
/// drive (`C:`), are [`Error::FileNotOnServer`].
fn descend<'a>(mut base: PathBuf, parts: impl Iterator<Item = &'a str>) -> Result<PathBuf, Error> {
    for part in parts {
        if part == ".." || part.contains(':') {
            return Err(Error::FileNotOnServer);
        }
        base = in_any_case(base.join(part));
    }
    Ok(base)
}

/// `name`, a path inside `base` as a client names it (a table, or the
/// folder of a table listing). An absolute path is
/// [`Error::FileNotOnServer`].
pub(crate) fn inside(base: &Path, name: &str) -> Result<PathBuf, Error> {
    let absolute = name.starts_with(['\\', '/']) || name.contains(':');
    if absolute {
        return Err(Error::FileNotOnServer);
    }
    descend(base.to_path_buf(), parts(name))
}

/// `path` as it exists, its last part matched regardless of case; as it is
/// if it exists or nothing matches.
pub(crate) fn in_any_case(path: PathBuf) -> PathBuf {
    if path.exists() {
        return path;
    }
    actual_case(path)
}

/// `path` with its file name as the folder spells it, if a file there has
/// that name regardless of case; else `path`. (On a file system that
/// ignores case, `path` may exist as it is spelled.)
pub(crate) fn actual_case(path: PathBuf) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    let Some(name) = name.to_str().map(str::to_lowercase) else {
        return path;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return path;
    };
    entries
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.to_lowercase() == name)
        })
        .map_or(path, |entry| entry.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt")
    }

    /// `path` as compared: on Windows, which ignores case, a name is found
    /// as the client spells it, so regardless of case there.
    fn cased(path: Result<PathBuf, Error>) -> Result<PathBuf, Error> {
        path.map(|path| match cfg!(windows) {
            true => PathBuf::from(path.to_string_lossy().to_lowercase()),
            false => path,
        })
    }

    #[test]
    fn connection_paths() {
        let mut shares = Shares::new();
        shares.add("Fixtures", fixtures());
        assert_eq!(
            cased(shares.connection_path(r"fixtures\all_types")),
            cased(Ok(fixtures().join("all_types")))
        );
        assert_eq!(
            cased(shares.connection_path("FIXTURES/ALL_TYPES/")),
            cased(Ok(fixtures().join("all_types")))
        );
        assert_eq!(
            cased(shares.connection_path(r"fixtures")),
            cased(Ok(fixtures()))
        );
        assert_eq!(
            cased(shares.connection_path(r"fixtures\.\indexes")),
            cased(Ok(fixtures().join("indexes")))
        );
        for outside in [r"other\x", "", r"\", r"fixtures\..\..", r"fixtures\C:\x"] {
            assert_eq!(
                shares.connection_path(outside),
                Err(Error::FileNotOnServer),
                "{outside}"
            );
        }
    }

    #[test]
    fn mapped_absolute_paths() {
        let mut shares = Shares::new();
        shares.add("Fixtures", fixtures());
        shares.add("Indexes", fixtures().join("indexes"));
        shares.add_map(r"C:\Data\", "fixtures");
        shares.add_map("c:/data/indexes", "indexes");
        shares.add_map(r"D:\gone", "nosuch");
        let dir = fixtures().join("all_types");
        // The longest map that takes a path in wins.
        assert_eq!(
            cased(shares.inside(&dir, r"C:\DATA\indexes\Indexed.adt")),
            cased(Ok(fixtures().join("indexes/indexed.adt")))
        );
        assert_eq!(
            cased(shares.inside(&dir, r"c:/data/all_types/ALL_TYPES.ADT")),
            cased(Ok(dir.join("all_types.adt")))
        );
        assert_eq!(
            cased(shares.inside(&dir, r"C:\data")),
            cased(Ok(fixtures()))
        );
        // A relative name is inside the folder, as without maps.
        assert_eq!(
            shares.inside(&dir, "missing.adt"),
            Ok(dir.join("missing.adt"))
        );
        for outside in [
            r"C:\database\x.adt",
            r"C:\x.adt",
            r"C:\data\..\x.adt",
            r"D:\gone\x.adt",
            "/etc/passwd",
        ] {
            assert_eq!(
                shares.inside(&dir, outside),
                Err(Error::FileNotOnServer),
                "{outside}"
            );
        }
        assert_eq!(
            cased(shares.connection_path(r"C:\data\indexes")),
            cased(Ok(fixtures().join("indexes")))
        );
    }

    #[test]
    fn names_inside_a_folder() {
        let dir = fixtures().join("all_types");
        assert_eq!(
            cased(inside(&dir, "ALL_TYPES.ADT")),
            cased(Ok(dir.join("all_types.adt")))
        );
        assert_eq!(inside(&dir, "missing.adt"), Ok(dir.join("missing.adt")));
        assert_eq!(
            cased(inside(&fixtures(), r"INDEXES\Indexed.adt")),
            cased(Ok(fixtures().join("indexes/indexed.adt")))
        );
        for outside in [
            r"..\x.adt",
            r"\etc\passwd",
            "/etc/passwd",
            r"C:\x.adt",
            r"sub\..\..\x",
        ] {
            assert_eq!(
                inside(&dir, outside),
                Err(Error::FileNotOnServer),
                "{outside}"
            );
        }
    }
}
