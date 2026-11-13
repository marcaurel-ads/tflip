//! The option string of the backup and restore procedures
//! (`fixtures/backup/README.md`, "Options"): `;`-separated, keywords in any
//! case, the key trimmed and the value not.

use crate::{Error, ace_error};

/// Whether the call backs up or restores: `DontOverwrite` belongs to restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Backup,
    Restore,
}

/// A parsed option string.
#[derive(Debug, Clone, Default)]
pub(crate) struct Options {
    /// The tables named by `include=`: `None` without it, none at all for an
    /// empty list. A repeated `include=` replaces the one before.
    pub include: Option<Vec<String>>,
    pub exclude: Vec<String>,
    pub user: Option<String>,
    pub dd_password: Option<String>,
    /// `ArchiveFile=` or `ArchiveFileCompressed=` and its name; the second
    /// wins when both are given.
    pub archive: Option<Archive>,
    pub table_type_map: Option<String>,
    pub no_warnings: bool,
    pub force_archive_extract: bool,
    pub meta_only: bool,
    pub prepare_diff: bool,
    pub diff: bool,
    pub dont_overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Archive {
    pub name: String,
    pub compressed: bool,
}

fn invalid(text: &str) -> Error {
    ace_error::sql(5019, text)
}

impl Options {
    /// Parses `text` (a NULL option string is empty), as the original does.
    pub(crate) fn parse(text: &str, operation: Operation) -> Result<Options, Error> {
        let mut options = Options::default();
        let tokens: Vec<(usize, &str)> = {
            let mut at = 0;
            text.split(';')
                .map(|token| {
                    let start = at;
                    at += token.len() + 1;
                    (start, token)
                })
                .collect()
        };
        let last = tokens.len() - 1;
        for (i, &(start, token)) in tokens.iter().enumerate() {
            if token.trim().is_empty() && (i == 0 || i == last) {
                continue;
            }
            let unknown = || {
                invalid(&format!(
                    "Unknown backup/restore option encountered: {}",
                    &text[start..]
                ))
            };
            let (key, value) = match token.split_once('=') {
                Some((key, value)) => (key, Some(value)),
                None => (token, None),
            };
            let value_of = |missing: &str| match value {
                Some(value) => Ok(value),
                None => Err(invalid(missing)),
            };
            match key.trim().to_ascii_lowercase().as_str() {
                key @ ("include" | "exclude") => {
                    let value = value_of("missing = in table inclusion/exclusion list")?;
                    let names: Vec<String> = value
                        .split(',')
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect();
                    match key {
                        "include" => options.include = Some(names),
                        _ => options.exclude = names,
                    }
                }
                "user" => {
                    options.user = Some(value_of("missing = after the User option")?.to_owned());
                }
                "ddpassword" => options.dd_password = Some(value.unwrap_or_default().to_owned()),
                "archivefile" => {
                    let name = value_of("missing = after the ArchiveFile option")?;
                    if !matches!(&options.archive, Some(archive) if archive.compressed) {
                        options.archive = Some(Archive {
                            name: name.to_owned(),
                            compressed: false,
                        });
                    }
                }
                "archivefilecompressed" => {
                    let name = value_of("missing = after the ArchiveFileCompressed option")?;
                    options.archive = Some(Archive {
                        name: name.to_owned(),
                        compressed: true,
                    });
                }
                "tabletypemap" => {
                    options.table_type_map =
                        Some(value_of("missing = in table type map")?.to_owned());
                }
                "nowarnings" => options.no_warnings = true,
                "forcearchiveextract" => options.force_archive_extract = true,
                "metaonly" => options.meta_only = true,
                "preparediff" => options.prepare_diff = true,
                "diff" => options.diff = true,
                "dontoverwrite" => options.dont_overwrite = true,
                _ => return Err(unknown()),
            }
        }
        if operation == Operation::Backup {
            if options.dont_overwrite {
                return Err(invalid(
                    "DONTOVERWRITE option is only supported on restore operations.",
                ));
            }
            if options.prepare_diff && options.diff {
                return Err(invalid(
                    "Cannot specify PrepareDiff and Diff for the same backup operation.",
                ));
            }
        }
        Ok(options)
    }

    /// Whether the table `name` of a dictionary is wanted: named by
    /// `include=` (all are without it) and not by `exclude=`, whatever the
    /// case.
    pub(crate) fn wants(&self, name: &str) -> bool {
        let named = |list: &[String]| {
            list.iter()
                .any(|item| table_name_of(item).eq_ignore_ascii_case(name))
        };
        self.include.as_ref().is_none_or(|list| named(list)) && !named(&self.exclude)
    }
}

/// A table's name as an include or exclude list gives it: an `.adt` after it
/// is accepted.
pub(crate) fn table_name_of(item: &str) -> &str {
    match item.len().checked_sub(4) {
        Some(at) if item.is_char_boundary(at) && item[at..].eq_ignore_ascii_case(".adt") => {
            &item[..at]
        }
        _ => item,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(error: Error) -> String {
        match error {
            Error::Sql(sql) => sql.message,
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn unknown_text_is_the_rest_of_the_string() {
        let error = Options::parse("include=t_adt;;exclude=t_cdx", Operation::Backup).unwrap_err();
        assert!(text(error).ends_with("encountered: ;exclude=t_cdx"));
        let error = Options::parse(";;", Operation::Backup).unwrap_err();
        assert!(text(error).ends_with("encountered: ;"));
        assert!(Options::parse(";include=t_adt;", Operation::Backup).is_ok());
    }

    #[test]
    fn include_and_exclude() {
        let options =
            Options::parse("include=T_ADT,t_cdx.adt;exclude=t_cdx", Operation::Restore).unwrap();
        assert!(options.wants("t_adt"));
        assert!(!options.wants("t_cdx"));
        assert!(!options.wants("t_vfp"));
        let options = Options::parse("include=t_adt, t_ntx", Operation::Restore).unwrap();
        assert!(!options.wants("t_ntx"));
    }
}
