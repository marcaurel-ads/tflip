//! The collations a session builds character keys with, from the original
//! engine's collation files (see [`tflip_storage::collation`]): the static
//! ANSI collation (`ANSI_CHAR_SET` in `adslocal.cfg`; if it is empty or there
//! is no such file, the one the host's locale derives, see
//! [`crate::system_collation`]), the static OEM collation
//! (`OEM_CHAR_SET`, `extend.chr`) an OEM Clipper or FoxPro table weighs its
//! keys by, and the dynamic collations of `adscollate.adt`, which index
//! orders name.
//!
//! Without the files, keys are the bytes themselves (the default collation's
//! weights), and orders with a dynamic collation cannot have their keys built
//! ([`crate::Error::NotImplemented`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tflip_storage::collation::{Dynamic, Record, Static};

use crate::lock;

/// The collations of a session.
#[derive(Debug, Default)]
pub struct Collations {
    /// The folder of the collation files, if there is one.
    dir: Option<PathBuf>,
    /// The static ANSI collation, if it is not the default one (whose
    /// weights are the bytes themselves).
    ansi: Option<Static>,
    /// The static OEM collation (`OEM_CHAR_SET`, from `extend.chr`), if it is
    /// not the default one; an OEM Clipper or FoxPro table weighs its keys by
    /// it instead of the ANSI one.
    oem: Option<Static>,
    /// The dynamic collations read so far, by name in upper case; `None` for
    /// one the files lack.
    dynamic: Mutex<HashMap<String, Option<Arc<Dynamic>>>>,
    /// The repository's collations in the order they were first loaded (by
    /// name in upper case), which numbers them for `sp_GetCollationTable`.
    loaded: Mutex<Vec<String>>,
}

impl Collations {
    /// The collations of the files in `dir`, with the static ANSI collation
    /// `ANSI_CHAR_SET` and OEM collation `OEM_CHAR_SET` of the `adslocal.cfg`
    /// there (as the original engine's local server reads them). Where
    /// `ANSI_CHAR_SET` is empty the ANSI collation is the one the host's
    /// locale derives ([`crate::system_collation::host`]); the OEM one is
    /// the default where `OEM_CHAR_SET` is empty, and either is the default
    /// where its file lacks the one named.
    pub fn from_folder(dir: &Path) -> Collations {
        Collations::from_folder_for(dir, crate::system_collation::host())
    }

    /// [`Collations::from_folder`] on a host whose locale derives the static
    /// ANSI collation `system` (`None`: the default one).
    pub fn from_folder_for(dir: &Path, system: Option<Static>) -> Collations {
        let config = std::fs::read_to_string(dir.join("adslocal.cfg")).unwrap_or_default();
        let ansi = config_value(&config, "ANSI_CHAR_SET").unwrap_or_default();
        let oem = config_value(&config, "OEM_CHAR_SET").unwrap_or_default();
        let mut collations = Collations::new(dir, &ansi, &oem);
        if ansi.trim().is_empty() {
            collations.ansi = system;
        }
        collations
    }

    /// The collations without the original engine's files: the static ANSI
    /// collation `system` the host's locale derives (`None`: the default
    /// one), which needs none, and the default OEM collation.
    pub fn system(system: Option<Static>) -> Collations {
        Collations {
            ansi: system,
            ..Collations::default()
        }
    }

    /// The collations of the files in `dir`, with the static ANSI collation
    /// `ansi_char_set` (from `ansi.chr`) and OEM collation `oem_char_set`
    /// (from `extend.chr`); the default of each where its name is empty
    /// (whatever the host's locale).
    pub fn new(dir: &Path, ansi_char_set: &str, oem_char_set: &str) -> Collations {
        let ansi = static_collation(dir, "ansi.chr", 16, ansi_char_set);
        let oem = static_collation(dir, "extend.chr", 14, oem_char_set);
        Collations {
            dir: Some(dir.to_owned()),
            ansi,
            oem,
            dynamic: Mutex::default(),
            loaded: Mutex::default(),
        }
    }

    /// The static ANSI collation, if it is not the default one.
    pub fn ansi(&self) -> Option<&Static> {
        self.ansi.as_ref()
    }

    /// The static OEM collation, if it is not the default one.
    pub fn oem(&self) -> Option<&Static> {
        self.oem.as_ref()
    }

    /// The static collation an index weighs its keys by: the OEM one for an
    /// OEM table, else the ANSI one; `None` for the default (identity) weights.
    pub fn table_collation(&self, oem: bool) -> Option<&Static> {
        if oem {
            self.oem.as_ref()
        } else {
            self.ansi.as_ref()
        }
    }

    /// How `UPPER`/`LOWER` fold case under the table's static collation (the
    /// OEM one for an OEM table): the original engine's `UPPER(b)` is
    /// `weights[upper[b]]` and `LOWER(b)` is `weights[lower[b]]`. `None` under
    /// the default collation, where the folding is plain ASCII.
    pub fn case_fold(&self, oem: bool) -> Option<tflip_expr::CaseFold> {
        let collation = self.table_collation(oem)?;
        let fold =
            |table: &[u8; 256]| std::array::from_fn(|b| collation.weights[usize::from(table[b])]);
        Some(tflip_expr::CaseFold {
            upper: fold(&collation.upper),
            lower: fold(&collation.lower),
        })
    }

    /// The name of the VFP collation a FoxPro index names `short` (`GERMAN`)
    /// for the code page `code_page`, if the files have it.
    pub fn vfp_name(&self, short: &str, code_page: u32) -> Option<String> {
        let dir = self.dir.as_ref()?;
        Dynamic::vfp_name(&dir.join("adscollate.adt"), short, code_page)
    }

    /// Whether the files have the repository of dynamic collations
    /// (`adscollate.adt`).
    pub fn has_repository(&self) -> bool {
        self.dir
            .as_ref()
            .is_some_and(|dir| dir.join("adscollate.adt").exists())
    }

    /// The repository's spelling of the collation `name`, if it has it.
    pub fn canonical(&self, name: &str) -> Option<String> {
        Dynamic::canonical(self.dir.as_ref()?, name)
    }

    /// The dynamic collation `name`, if the files have it.
    pub fn dynamic(&self, name: &str) -> Option<Arc<Dynamic>> {
        let dir = self.dir.as_ref()?;
        let key = name.trim().to_ascii_uppercase();
        let mut cache = lock(&self.dynamic);
        let found = cache
            .entry(key.clone())
            .or_insert_with(|| Dynamic::open(&dir.join("adscollate.adt"), name).map(Arc::new))
            .clone();
        if found.is_some() {
            self.number(&key);
        }
        found
    }

    /// The records of the repository (`adscollate.adt`), in the file's order.
    pub fn records(&self) -> Vec<Record> {
        self.dir.as_ref().map_or_else(Vec::new, |dir| {
            Dynamic::records(&dir.join("adscollate.adt"))
        })
    }

    /// The number the engine gives the collation `name` once it has loaded
    /// it: 1 for the first it loads, then up (`sp_GetCollationTable`'s
    /// `CollationID`).
    pub fn number(&self, name: &str) -> i32 {
        let key = name.trim().to_ascii_uppercase();
        let mut loaded = lock(&self.loaded);
        let at = loaded.iter().position(|n| *n == key).unwrap_or_else(|| {
            loaded.push(key);
            loaded.len() - 1
        });
        at as i32 + 1
    }
}

/// The static collation `name` of the `.chr` `file` in `dir` (whose directory
/// entries are `entry` bytes long), or the default (identity) one, which it
/// returns as `None`: an empty name, no file, or a collation `file` lacks all
/// give the default, and a named one that happens to weigh every byte as
/// itself is the default too.
fn static_collation(dir: &Path, file: &str, entry: usize, name: &str) -> Option<Static> {
    (!name.trim().is_empty())
        .then(|| std::fs::read(dir.join(file)).ok())
        .flatten()
        .and_then(|chr| Static::from_chr(&chr, entry, name))
        .filter(|c| {
            c.weights
                .iter()
                .enumerate()
                .any(|(i, &w)| usize::from(w) != i)
        })
}

/// The value of `key` (regardless of case) in an ini-style file.
fn config_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with(';'))
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case(key)
                .then(|| value.trim().to_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collations_are_numbered_as_they_are_loaded() {
        let collations = Collations::default();
        assert_eq!(collations.number("German_VFP_CI_AS_1252"), 1);
        assert_eq!(collations.number("DUTCH_VFP_CI_AS_1252"), 2);
        assert_eq!(collations.number(" german_vfp_ci_as_1252"), 1);
        assert_eq!(collations.number("CZECH_VFP_CI_AS_1250"), 3);
    }

    #[test]
    fn configured_values() {
        let text = "[SETTINGS]\n; ANSI_CHAR_SET=no\nCONNECTIONS=20\nansi_char_set = German\n";
        assert_eq!(
            config_value(text, "ANSI_CHAR_SET").as_deref(),
            Some("German")
        );
        assert_eq!(config_value(text, "OEM_CHAR_SET"), None);
        let none = Collations::default();
        assert!(none.ansi().is_none());
        assert!(none.oem().is_none());
        assert!(none.table_collation(true).is_none());
        assert!(none.table_collation(false).is_none());
        assert!(none.dynamic("GERMAN_VFP_CI_AS_1252").is_none());
    }
}
