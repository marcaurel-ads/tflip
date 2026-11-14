//! A session's settings: what ACE keeps process-wide (`AdsSetDateFormat`,
//! `AdsSetExact`, ...), kept per [`Session`] so a server's clients do not
//! share them.

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use crate::collations::Collations;
use crate::handle::{Handle, Object};
use crate::{Error, Session, lock};

/// The settings of a session. Connections and tables keep a reference to
/// them for what they look up later, such as the date format.
#[derive(Debug)]
pub struct Settings {
    /// The date format text getters use, as set by the client; empty for
    /// ACE's default `MM/DD/CCYY`. See [`crate::format::format_date`].
    date_format: Mutex<String>,
    /// The `ADS_*_SERVER` bits of the servers `AdsConnect` may connect to,
    /// as set by `AdsSetServerType`; 0 until set.
    server_types: AtomicU16,
    /// The "exact" setting: whether string comparisons with `=`, `<`, `>`,
    /// `<=` and `>=` in filters ignore trailing spaces. Off until set.
    exact: AtomicBool,
    /// Whether deleted records of DBF tables are visible: visible until set,
    /// as in the original engine.
    show_deleted: AtomicBool,
    /// The number of decimals for converting numbers to text: 2 until
    /// changed, as in the original engine. It does not apply to Double
    /// fields, which use their own decimals.
    decimals: AtomicU16,
    /// The epoch of years of two digits (`AdsSetEpoch`): 1900 until set.
    epoch: AtomicU16,
    /// The collations keys are built with.
    collations: Mutex<Arc<Collations>>,
    /// The search path (`AdsSetSearchPath`), where the original engine also
    /// looks for tables; kept, but not looked in. Empty until set.
    search_path: Mutex<String>,
    /// The default path (`AdsSetDefault`), where the original engine opens
    /// and creates tables given without a path or a connection; kept, but
    /// not looked in. Empty until set.
    default_path: Mutex<String>,
    /// Whether transactions are real ones (see [`real_transactions`]).
    real_transactions: AtomicBool,
    /// Whether relative names are relative to the process's current
    /// directory (see [`cwd_relative`]).
    cwd_relative: AtomicBool,
    /// The sizes of the caches (`[tables, statements]`) of the connections
    /// of a session in a client's own process, if they keep closed tables
    /// open (see [`set_local_caching`]); `None`, keeping none, until set.
    local_caches: Mutex<Option<[u16; 2]>>,
}

impl Settings {
    /// Whether deleted records of DBF tables are shown (`AdsShowDeleted`).
    pub(crate) fn shows_deleted(&self) -> bool {
        self.show_deleted.load(Ordering::Relaxed)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            date_format: Mutex::new(String::new()),
            server_types: AtomicU16::new(0),
            exact: AtomicBool::new(false),
            show_deleted: AtomicBool::new(true),
            decimals: AtomicU16::new(2),
            epoch: AtomicU16::new(tflip_expr::text::DEFAULT_EPOCH),
            collations: Mutex::default(),
            search_path: Mutex::default(),
            default_path: Mutex::default(),
            real_transactions: AtomicBool::new(true),
            cwd_relative: AtomicBool::new(false),
            local_caches: Mutex::new(None),
        }
    }
}

impl Settings {
    /// Whether transactions are real ones (see [`real_transactions`]).
    pub(crate) fn real_transactions_on(&self) -> bool {
        self.real_transactions.load(Ordering::Relaxed)
    }

    /// The collations keys are built with.
    pub fn collations(&self) -> Arc<Collations> {
        Arc::clone(&lock(&self.collations))
    }

    /// The epoch of years of two digits.
    pub fn epoch(&self) -> u16 {
        self.epoch.load(Ordering::Relaxed)
    }

    /// The date format text getters use.
    pub fn date_format(&self) -> String {
        let format = lock(&self.date_format);
        if format.is_empty() {
            "MM/DD/CCYY".to_owned()
        } else {
            format.clone()
        }
    }
}

/// Sets the collations the session builds keys with (from the original
/// engine's collation files: see [`Collations`]).
pub fn set_collations(session: &Session, collations: Collations) {
    *lock(&session.settings.collations) = Arc::new(collations);
}

/// Gives `session` the collations of `from`.
pub(crate) fn share_collations(session: &Session, from: &Session) {
    *lock(&session.settings.collations) = from.settings.collations();
}

/// The session's epoch: the first year of the hundred that years of two
/// digits fall in (`AdsGetEpoch`).
pub fn epoch(session: &Session) -> u16 {
    session.settings.epoch()
}

/// The session's search path (`AdsGetSearchPath`); empty until set.
pub fn search_path(session: &Session) -> String {
    lock(&session.settings.search_path).clone()
}

/// Sets the session's search path (`AdsSetSearchPath`).
pub fn set_search_path(session: &Session, path: &str) {
    *lock(&session.settings.search_path) = path.to_owned();
}

/// The session's default path (`AdsGetDefault`); empty until set.
pub fn default_path(session: &Session) -> String {
    lock(&session.settings.default_path).clone()
}

/// Sets the session's default path (`AdsSetDefault`): as in the original
/// engine, a path gets a `\` at its end, and an empty one clears it.
pub fn set_default_path(session: &Session, path: &str) {
    let mut path = path.to_owned();
    if !path.is_empty() && !path.ends_with('\\') {
        path.push('\\');
    }
    *lock(&session.settings.default_path) = path;
}

/// Sets the session's epoch (`AdsSetEpoch`); as in the original engine,
/// any value is taken.
pub fn set_epoch(session: &Session, epoch: u16) {
    session.settings.epoch.store(epoch, Ordering::Relaxed);
}

/// The session's date format text getters use.
pub fn date_format(session: &Session) -> String {
    session.settings.date_format()
}

/// The longest date format the original engine accepts, in characters.
const DATE_FORMAT_MAX: usize = 11;

/// Sets the date format text getters use, as the original engine accepts
/// it: at most 11 characters ([`Error::DataTooLong`]), with exactly one day
/// (`DD`), one month (`MM`) and one year (`YY`, or `CCYY` and its spelling
/// `YYYY`), in any order and either case. Every other run of `D`, `M`, `Y`
/// or `C` (`M/d/yyyy`, `MMM`, `YYY`, `CC`), a field given twice or missing,
/// is [`Error::InvalidDateFormat`]. Other characters are kept as literals.
/// The format is kept in upper case, with `YYYY` as `CCYY`.
///
/// As in the original engine, it is every connection's date format from
/// then on, also of those that had one of their own
/// ([`crate::connection::set_date_format`]).
pub fn set_date_format(session: &Session, format: &str) -> Result<(), Error> {
    *lock(&session.settings.date_format) = parse_date_format(format)?;
    crate::connection::forget_date_formats(session);
    Ok(())
}

/// `format` as [`set_date_format`] keeps it, or why it is refused.
pub(crate) fn parse_date_format(format: &str) -> Result<String, Error> {
    if format.chars().count() > DATE_FORMAT_MAX {
        return Err(Error::DataTooLong);
    }
    let chars: Vec<char> = format.chars().map(|c| c.to_ascii_uppercase()).collect();
    let (mut day, mut month, mut year) = (false, false, false);
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        let kind = chars[i];
        let same = |c: char| match kind {
            'Y' | 'C' => matches!(c, 'Y' | 'C'),
            _ => c == kind,
        };
        let run: String = match kind {
            'D' | 'M' | 'Y' | 'C' => chars[i..].iter().take_while(|&&c| same(c)).collect(),
            _ => kind.to_string(),
        };
        let seen = match run.as_str() {
            "DD" => &mut day,
            "MM" => &mut month,
            "YY" | "YYYY" | "CCYY" => &mut year,
            _ if matches!(kind, 'D' | 'M' | 'Y' | 'C') => return Err(Error::InvalidDateFormat),
            _ => {
                text.push(kind);
                i += 1;
                continue;
            }
        };
        if std::mem::replace(seen, true) {
            return Err(Error::InvalidDateFormat);
        }
        text += if run == "YYYY" { "CCYY" } else { &run };
        i += run.chars().count();
    }
    if !(day && month && year) {
        return Err(Error::InvalidDateFormat);
    }
    Ok(text)
}

/// The servers connecting by path may use when it does not say
/// (`AdsSetServerType`): the `ADS_*_SERVER` bits. Until set, remote and
/// Internet servers (`ADS_REMOTE_SERVER | ADS_AIS_SERVER`), as in the
/// original engine.
pub fn server_types(session: &Session) -> u16 {
    match session.settings.server_types.load(Ordering::Relaxed) {
        0 => crate::connection::REMOTE | crate::connection::AIS,
        types => types,
    }
}

/// Sets the servers connecting by path may use when it does not say.
pub fn set_server_types(session: &Session, types: u16) {
    session
        .settings
        .server_types
        .store(types, Ordering::Relaxed);
}

/// The session's "exact" setting (`AdsGetExact`), which new connections
/// start with. Connections and tables have their own ([`exact_of`]). Nothing
/// evaluates filters yet, so it changes nothing so far.
pub fn exact(session: &Session) -> bool {
    session.settings.exact.load(Ordering::Relaxed)
}

/// Sets the session's "exact" setting (`AdsSetExact`). As in the original
/// engine, it sets every open connection's and table's as well.
pub fn set_exact(session: &Session, exact: bool) {
    session.settings.exact.store(exact, Ordering::Relaxed);
    session.for_each(|object| match object {
        Object::Connection(connection) => connection.exact.store(exact, Ordering::Relaxed),
        Object::Table(table) => table.exact.store(exact, Ordering::Relaxed),
        _ => {}
    });
}

/// The "exact" setting of a connection or table (`AdsGetExact22`); handle 0
/// is the session's. Any other handle is [`Error::InvalidHandle`].
pub fn exact_of(session: &Session, handle: Handle) -> Result<bool, Error> {
    if handle == 0 {
        return Ok(exact(session));
    }
    session
        .with(handle, |object| match object {
            Object::Connection(connection) => Some(connection.exact.load(Ordering::Relaxed)),
            Object::Table(table) => Some(table.exact.load(Ordering::Relaxed)),
            _ => None,
        })
        .flatten()
        .ok_or(Error::InvalidHandle)
}

/// Sets the "exact" setting of a connection or table (`AdsSetExact22`);
/// handle 0 sets the session's ([`set_exact`]). As in the original engine, a
/// connection's setting is only where its tables opened from then on start;
/// the tables open on it keep theirs. Any other handle is
/// [`Error::InvalidHandle`].
pub fn set_exact_of(session: &Session, handle: Handle, exact: bool) -> Result<(), Error> {
    if handle == 0 {
        set_exact(session, exact);
        return Ok(());
    }
    let set = session.with(handle, |object| {
        let setting = match object {
            Object::Connection(connection) => &connection.exact,
            Object::Table(table) => &table.exact,
            _ => return false,
        };
        setting.store(exact, Ordering::Relaxed);
        true
    });
    if set == Some(true) {
        Ok(())
    } else {
        Err(Error::InvalidHandle)
    }
}

/// Whether deleted records are visible.
pub fn show_deleted(session: &Session) -> bool {
    session.settings.show_deleted.load(Ordering::Relaxed)
}

/// Whether transactions are real ones (the default), or only counted, as
/// the original's local server keeps them (see [`crate::transaction`]).
pub fn real_transactions(session: &Session) -> bool {
    session.settings.real_transactions.load(Ordering::Relaxed)
}

/// Makes transactions real ones, or only counted.
pub fn set_real_transactions(session: &Session, real: bool) {
    session
        .settings
        .real_transactions
        .store(real, Ordering::Relaxed);
}

/// Whether relative table names, listing masks and index paths with a
/// folder are relative to the process's current directory, as in the
/// original's local server, instead of to the connection's folder (the
/// default; `docs/DIFFERENCES.md`). Only a session without shares (a
/// client's own process) has a current directory to go by.
pub fn cwd_relative(session: &Session) -> bool {
    session.shares.is_none() && session.settings.cwd_relative.load(Ordering::Relaxed)
}

/// Makes relative names relative to the current directory, or to the
/// connection's folder.
pub fn set_cwd_relative(session: &Session, on: bool) {
    session.settings.cwd_relative.store(on, Ordering::Relaxed);
}

/// `name` in `folder`, a connection's, in a session without shares: in
/// the current directory instead where [`cwd_relative`] says so (an
/// absolute `name` is itself either way).
pub(crate) fn local_join(
    session: &Session,
    folder: &std::path::Path,
    name: &str,
) -> std::path::PathBuf {
    match cwd_relative(session) {
        true => std::env::current_dir()
            .unwrap_or_else(|_| folder.to_path_buf())
            .join(name),
        false => folder.join(name),
    }
}

/// Makes the connections of `session`, a client's own process's, keep
/// closed tables open for reuse as a server's do (see `crate::cache`): no
/// tables and the tables of 25 statements until `AdsCacheOpenTables` and
/// `AdsCacheOpenCursors` ([`set_cache_sizes`]) say otherwise, as with the
/// original's local server. Off, as by default, they keep none, and close
/// what they kept. A server's sessions always keep them.
pub fn set_local_caching(session: &Session, on: bool) {
    let sizes = {
        let mut caches = lock(&session.settings.local_caches);
        match (on, *caches) {
            (true, Some(_)) => return,
            (true, None) => *caches = Some([0, crate::connection::DEFAULT_CACHED_CURSORS]),
            (false, _) => *caches = None,
        }
        caches.unwrap_or([0, 0])
    };
    for connection in crate::connection::all(session) {
        for (cursors, size) in [false, true].into_iter().zip(sizes) {
            connection.set_cache_size(cursors, size, true);
        }
    }
}

/// The sizes of the caches (`[tables, statements]`) a new connection of
/// `session`, a client's own process's, takes, if its caching is on (see
/// [`set_local_caching`]).
pub(crate) fn local_cache_sizes(session: &Session) -> Option<[u16; 2]> {
    *lock(&session.settings.local_caches)
}

/// Sets how many closed tables (`cursors` false: `AdsCacheOpenTables`) or
/// SQL statements' tables (`AdsCacheOpenCursors`) every connection of
/// `session`, a client's own process's, keeps open, now and for the
/// connections made later, as the original engine's settings are the
/// process's. Unless its caching is on (see [`set_local_caching`]), its
/// connections keep none, and this does nothing.
pub fn set_cache_sizes(session: &Session, cursors: bool, size: u16) {
    {
        let mut caches = lock(&session.settings.local_caches);
        let Some(sizes) = caches.as_mut() else { return };
        sizes[usize::from(cursors)] = size;
    }
    for connection in crate::connection::all(session) {
        connection.set_cache_size(cursors, size, session.shares.is_none());
    }
}

/// Makes deleted records visible or hidden.
pub fn set_show_deleted(session: &Session, show: bool) {
    session.settings.show_deleted.store(show, Ordering::Relaxed);
}

/// The number of decimals for converting numbers to text.
pub fn decimals(session: &Session) -> u16 {
    session.settings.decimals.load(Ordering::Relaxed)
}

/// Sets the number of decimals for converting numbers to text.
pub fn set_decimals(session: &Session, decimals: u16) {
    session.settings.decimals.store(decimals, Ordering::Relaxed);
}

/// Sets how the text a client wrote in a SQL statement becomes the OEM text
/// of the OEM tables it meets (the process's one: the client's code pages).
pub fn set_oem_text(convert: fn(&str) -> String, back: fn(&str) -> String) {
    tflip_sql::compile::set_oem_text(convert, back);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With `[paths] relative = cwd`, a relative name on a session without
    /// shares is in the current directory; an absolute one is itself.
    #[test]
    fn relative_names_in_the_current_directory() {
        let session = Session::new();
        let folder = std::path::Path::new("/data/app");
        assert_eq!(local_join(&session, folder, "t.adt"), folder.join("t.adt"));
        set_cwd_relative(&session, true);
        assert!(cwd_relative(&session));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(local_join(&session, folder, "t.adt"), cwd.join("t.adt"));
        assert_eq!(
            local_join(&session, folder, "sub/t.adt"),
            cwd.join("sub/t.adt")
        );
        assert_eq!(
            local_join(&session, folder, "/x/t.adt"),
            std::path::PathBuf::from("/x/t.adt")
        );
        assert!(!cwd_relative(&Session::new()), "other sessions keep theirs");
    }

    #[test]
    fn date_format_round_trip() {
        let session = Session::new();
        assert_eq!(date_format(&session), "MM/DD/CCYY");
        assert_eq!(set_date_format(&session, "dd.mm.yyyy"), Ok(()));
        assert_eq!(date_format(&session), "DD.MM.CCYY");
        assert_eq!(
            set_date_format(&session, "M/d/yyyy"),
            Err(Error::InvalidDateFormat)
        );
        assert_eq!(
            date_format(&session),
            "DD.MM.CCYY",
            "a refused format changes nothing"
        );
        assert_eq!(
            date_format(&Session::new()),
            "MM/DD/CCYY",
            "other sessions keep theirs"
        );
        assert_eq!(set_date_format(&session, "MM/DD/CCYY"), Ok(()));
        assert_eq!(date_format(&session), "MM/DD/CCYY");
    }

    /// The formats as the original engine accepts and keeps them, or refuses
    /// them.
    #[test]
    fn date_formats_match_the_engine() {
        let accepted = [
            ("MM/DD/YYYY", "MM/DD/CCYY"),
            ("mm/dd/yyyy", "MM/DD/CCYY"),
            ("dd.MM.yyyy", "DD.MM.CCYY"),
            ("YYYYMMDD", "CCYYMMDD"),
            ("YY-MM-DD", "YY-MM-DD"),
            ("DDMMYY", "DDMMYY"),
            ("YY/DD/MM", "YY/DD/MM"),
            ("MM-DD/YY", "MM-DD/YY"),
            ("MMxDDxYY", "MMXDDXYY"),
            ("MM/DD/YY h", "MM/DD/YY H"),
            ("Q MM/DD/YY", "Q MM/DD/YY"),
            ("  MM/DD/YY", "  MM/DD/YY"),
            ("MM/DD/YYYY ", "MM/DD/CCYY "),
        ];
        for (format, kept) in accepted {
            assert_eq!(parse_date_format(format).as_deref(), Ok(kept), "{format:?}");
        }
        let refused = [
            "M/D/YYYY",
            "M/d/yyyy",
            "d/M/yyyy",
            "MM/D/YYYY",
            "D.M.YYYY",
            "DD-MMM-YYYY",
            "MMMM",
            "dddd",
            "YYYY",
            "MM/DD",
            "MM/DD/YYYYY",
            "MM/DD/YYY",
            "MM/DD/CC",
            "CC/MM/DD",
            "MM/DD/CY",
            "MM/DD/C",
            "DD/DD/YYYY",
            "MM/MM/YYYY",
            "MM/DD/YY/YY",
            "x",
            "",
        ];
        for format in refused {
            assert_eq!(
                parse_date_format(format),
                Err(Error::InvalidDateFormat),
                "{format:?}"
            );
        }
        for format in [
            "MM/DD/YYYY  ",
            "MMM DD, YYYY",
            "D. MMMM YYYY",
            "MM/DD/YYYY hh:mm",
        ] {
            assert_eq!(
                parse_date_format(format),
                Err(Error::DataTooLong),
                "{format:?}"
            );
        }
    }

    #[test]
    fn show_deleted_round_trip() {
        let session = Session::new();
        set_show_deleted(&session, false);
        assert!(!show_deleted(&session));
        set_show_deleted(&session, true);
        assert!(show_deleted(&session));
    }
}
