//! Creating a dictionary, and adding tables and index files to it.

use super::*;

/// Creates the data dictionary `AdsDDCreate` names (a path ending in `.add`,
/// or given that extension), writes its files as the original engine does —
/// the `.add` with its one Database object, the `.am` memo and the `.ai`
/// index of the dictionary's system orders — and connects to it as `ADSSYS`,
/// returning the connection. A file there already is [`Error::FileCreate`].
/// A non-zero `encrypt` (`AdsDDCreate`'s flag; the original takes any value
/// as 1) makes a dictionary that encrypts its records with RC4, as one with
/// the fixed key (`fixtures/aes/dictwrite`, `22_rc4_create`).
pub fn create_dictionary_api(session: &Session, path: &str, encrypt: u16) -> Result<Handle, Error> {
    let add_path = {
        let mut path = PathBuf::from(path);
        if path.extension().is_none() {
            path.set_extension("add");
        }
        path
    };
    if add_path.exists() {
        return Err(already_exists());
    }
    write_dictionary(session, &add_path, "", None, encrypt != 0)?;
    connection::connect_path(
        session,
        &add_path.to_string_lossy(),
        connection::LOCAL,
        Some("ADSSYS"),
        None,
    )
}

/// 5128, what a dictionary to create that is there already is refused with.
fn already_exists() -> Error {
    Error::Constraint {
        code: 5128,
        text: ace_error::error_text(5128).unwrap_or_default().to_owned(),
    }
}

/// The keys of a connect string `AdsDDCreate101` accepts, as probed (every
/// other one is refused with 5019 naming it, before anything is checked or
/// written; `AdsConnect101` refuses them the same way).
const CONNECT_KEYS: [&str; 20] = [
    "data source",
    "datasource",
    "servertype",
    "user id",
    "userid",
    "password",
    "tabletype",
    "chartype",
    "lockmode",
    "trimtrailingspaces",
    "compression",
    "commtype",
    "encryptionpassword",
    "encryptiontype",
    "fips",
    "ddpassword",
    "collation",
    "decimals",
    "epoch",
    "description",
];

/// The first key of `connect_string` that is not one of [`CONNECT_KEYS`], as
/// the original words it: the key as written, or for an item with no `=`
/// the item with the `;` after it, and for an empty key the value with it.
fn unknown_key(connect_string: &str) -> Option<String> {
    let mut items = connect_string.split(';').peekable();
    while let Some(item) = items.next() {
        let closed = if items.peek().is_some() { ";" } else { "" };
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        let key = key.trim();
        if key.is_empty() && value.is_empty() {
            continue;
        }
        if CONNECT_KEYS
            .iter()
            .any(|known| key.eq_ignore_ascii_case(known))
        {
            continue;
        }
        return Some(match (item.contains('='), key.is_empty()) {
            (false, _) => format!("{key}{closed}"),
            (true, true) => format!("{}{closed}", value.trim()),
            (true, false) => key.to_owned(),
        });
    }
    None
}

/// Creates the data dictionary `AdsDDCreate101` names with its connect
/// string (`fixtures/aes/dictwrite`, `14_ddcreate`): its `Data Source` is the
/// path; `EncryptionType=AES128` or `AES256` makes it an AES dictionary, its
/// key the `DDPassword` of 1 to 20 characters, and `RC4` or no type a plain
/// one. The connection is to it as `ADSSYS`, with the password.
///
/// An AES dictionary with no `DDPassword`, or one of more than 20
/// characters, is refused (5223, 5005) after the original has written its
/// `.add`'s header and its `.am`, which stay and make a retry find the
/// dictionary there (5128). The type is checked before all of it.
pub fn create_dictionary_connect_api(
    session: &Session,
    connect_string: &str,
) -> Result<Handle, Error> {
    if let Some(key) = unknown_key(connect_string) {
        return Err(Error::InvalidOption.detailed(key));
    }
    let value = |key| connection::connect_string_value(connect_string, key);
    let description = value("description").unwrap_or("");
    let kind = match value("encryptiontype") {
        None => None,
        Some(kind) if kind.eq_ignore_ascii_case("RC4") => None,
        Some(kind) if kind.eq_ignore_ascii_case("AES128") => {
            Some(tflip_storage::adt::ENCRYPTION_AES128)
        }
        Some(kind) if kind.eq_ignore_ascii_case("AES256") => {
            Some(tflip_storage::adt::ENCRYPTION_AES256)
        }
        Some("") => return Err(Error::InvalidOption.detailed("EncryptionType")),
        Some(kind) => return Err(Error::InvalidOption.detailed(kind)),
    };
    let Some(kind) = kind else {
        let add_path = dictionary_path(value("data source").unwrap_or(""));
        if add_path.exists() {
            return Err(already_exists());
        }
        write_dictionary(session, &add_path, description, None, false)?;
        let handle = connection::connect_path(
            session,
            &add_path.to_string_lossy(),
            connection::LOCAL,
            Some("ADSSYS"),
            None,
        )?;
        return administered(session, handle, &add_path, value("password"));
    };
    let password = match value("ddpassword") {
        Some("") => return Err(Error::InvalidOption.detailed("DDPassword")),
        password => password,
    };
    let add_path = dictionary_path(value("data source").unwrap_or(""));
    if add_path.exists() {
        return Err(already_exists());
    }
    let Some(password) = password else {
        let refused = Error::Constraint {
            code: 5223,
            text: "The requested operation requires a password.  When using strong \
                   encryption, you must specify the DDPassword connection string property."
                .to_owned(),
        };
        return Err(unkeyed(&add_path, kind, refused));
    };
    if password.len() > 20 {
        return Err(unkeyed(&add_path, kind, Error::FullText(5005)));
    }
    let aes = NewAes {
        kind,
        password,
        encrypt: false,
    };
    write_dictionary(session, &add_path, description, Some(aes), false)?;
    let handle = connection::open(
        session,
        add_path.clone(),
        connection::ServerType::Local,
        Some("ADSSYS"),
        None,
        Some(password),
        kind,
    )?;
    administered(session, handle, &add_path, value("password"))
}

/// Gives the new dictionary the `Password` of its connect string as the
/// administrator's (`fixtures/aes/dictwrite`, `46_connect_keys`): whatever
/// the `User ID` is, and an empty one leaves none.
fn administered(
    session: &Session,
    handle: Handle,
    add_path: &Path,
    password: Option<&str>,
) -> Result<Handle, Error> {
    if let Some(password) = password.filter(|password| !password.is_empty()) {
        let conn = connection::get(session, handle)?;
        super::users::write_admin_password(session, &conn, add_path, password.as_bytes())?;
    }
    Ok(handle)
}

/// The `.add` a dictionary's path names: the path, given that extension if
/// it has none.
fn dictionary_path(path: &str) -> PathBuf {
    let mut path = PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension("add");
    }
    path
}

/// `error`, after the half-made dictionary the original leaves when it
/// refuses an AES dictionary's password: the `.add` with its AES layout
/// and header but no key and no record, and the `.am`, unless the files are
/// there already.
fn unkeyed(add_path: &Path, kind: u8, error: Error) -> Error {
    use tflip_storage::dd;
    if !add_path.exists() {
        let _ = (|| -> Result<(), dd::Error> {
            let header = dd::create::unkeyed_aes_dictionary_add(&login_salts(), kind)?;
            std::fs::write(dd::memo_path(add_path), dd::create::new_dictionary_memo())?;
            Ok(std::fs::write(add_path, header)?)
        })();
    }
    error
}

/// What makes a new dictionary an AES one: the encryption type of its
/// tables (5 or 6), its dictionary password, and whether it encrypts its
/// records.
pub(crate) struct NewAes<'a> {
    pub(crate) kind: u8,
    pub(crate) password: &'a str,
    pub(crate) encrypt: bool,
}

/// Writes the files of a new data dictionary, the `.add` at `add_path` with
/// its one Database object (the `description` in its comment), the `.am`
/// memo and the `.ai` index of its system orders; an AES one with `aes`,
/// whose dictionary password the process then keeps for its writes, or, with
/// `rc4`, one that encrypts its records with RC4.
pub(super) fn write_dictionary(
    session: &Session,
    add_path: &Path,
    description: &str,
    aes: Option<NewAes>,
    rc4: bool,
) -> Result<(), Error> {
    use tflip_storage::dd;
    let salts = login_salts();
    let add = match &aes {
        None if rc4 => dd::create::new_rc4_dictionary_add(&salts),
        None => dd::create::new_dictionary_add(&salts),
        Some(aes) => {
            let material = tflip_storage::create::AesMaterial::generate(aes.kind)
                .map_err(|_| Error::FileCreate)?;
            dd::create::new_aes_dictionary_add(
                &salts,
                &dd::create::NewAes {
                    material,
                    password: aes.password.as_bytes(),
                    encrypt: aes.encrypt,
                },
            )
        }
    }
    .map_err(|_| Error::FileCreate)?;
    let memo = dd::memo_path(add_path);
    std::fs::write(&memo, dd::create::new_dictionary_memo()).map_err(|_| Error::FileCreate)?;
    std::fs::write(add_path, &add).map_err(|_| Error::FileCreate)?;
    if let Some(aes) = &aes {
        dd::unlock(add_path, aes.password.as_bytes()).map_err(|_| Error::FileCreate)?;
    }
    if !description.is_empty() {
        let mut file = dd::DictionaryFile::open(add_path).map_err(|_| Error::FileCreate)?;
        let mut record = file.record(1).map_err(|_| Error::FileCreate)?;
        dd::create::set_comment_strlen(&mut record, &memo, description.as_bytes())
            .map_err(|_| Error::FileCreate)?;
        file.write(1, &record).map_err(|_| Error::FileCreate)?;
    }

    // Build the dictionary's index over its one object.
    let table = Arc::new(table::dictionary(session, add_path)?);
    let weights = table
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = create::collation_crc(weights.as_ref());
    index::build_dictionary_index(&table, &add_path.with_extension("ai"), collation)
}

/// The data type a dictionary records for an index key (`ADS_*`), from the
/// type its expression evaluates to.
pub(super) fn ads_key_type(ty: tflip_expr::Type) -> u16 {
    use tflip_expr::Type;
    match ty {
        Type::Logical => 1,        // ADS_LOGICAL
        Type::Date => 3,           // ADS_DATE
        Type::String { .. } => 4,  // ADS_STRING
        Type::Binary { .. } => 16, // ADS_RAW
        _ => 2,                    // ADS_NUMERIC (numbers, time, timestamp)
    }
}

/// Adds an existing table to the data dictionary `connection` is to
/// (`AdsDDAddTable`): writes a Table object and a Field object per column into
/// the `.add`, an Index object per `index_files` entry with a Key object per
/// tag, and rebuilds the dictionary index. `table_path` is the table file,
/// relative to the dictionary's folder; the table and its index files must
/// exist. An ADT table is bound to the dictionary (its header names it); a DBF
/// table (`TableType::Ntx`, `Cdx`, `Vfp`) is left as it is, its type kept in
/// the Table object's `Info1`, its character type (`ADS_OEM` if `char_type` is
/// 2) in its `Info2` and its VFP field flags in the Field objects' options,
/// as the original engine writes them (`fixtures/dictionary/dbftables`,
/// `dbfcorners`). The table type `Database` (`ADS_DEFAULT`) is no type to add
/// ([`Error::InvalidTableType`]), and an ADT table has no character type.
#[allow(clippy::too_many_arguments)]
pub fn add_table_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    table_path: &str,
    table_type: TableType,
    char_type: u16,
    index_files: &str,
    _comments: &str,
) -> Result<(), Error> {
    use tflip_storage::dbf::Flavour;
    use tflip_storage::dd::{self, create::NewObject};
    use tflip_storage::table::Format;
    let format = match table_type {
        TableType::Adt => Format::Adt,
        TableType::Ntx => Format::Dbf(Flavour::Clipper),
        TableType::Cdx => Format::Dbf(Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(Flavour::Visual),
        TableType::Database => return Err(Error::InvalidTableType),
    };
    let conn = connection::get(session, connection)?;
    if !conn.dictionary {
        return Err(Error::InvalidConnection);
    }
    super::rights::require_create(&conn, 1, "table")?;
    let add_path = conn.path.clone();
    let folder = dictionary_folder(&add_path).to_path_buf();

    // The table being added, opened to read its fields and compile its index
    // expressions; it must exist. A table that is bound to a dictionary is
    // not added to one (5130), but the original opens it first, so a table
    // whose structure it cannot read says why instead: 5138 with no key for
    // it, 7060 with another RC4 one, 7068 for an AES table in a dictionary of
    // RC4 tables (`fixtures/aes/rc4tp`, `z1`, `z2`).
    let path = folder.join(table_path);
    if table_type == TableType::Adt {
        check_not_bound(&conn, &path)?;
    }
    let source = Arc::new(
        table::transient(session, &path, format)
            .map_err(|_| Error::FileNotFound.detailed(path.display().to_string()))?,
    );
    let taken = conn
        .with_database_opt(|db| {
            db.dictionary()
                .named(tflip_storage::dd::Kind::Table, table_name)
                .is_some()
        })
        .unwrap_or(false);
    if taken {
        // The original has taken the table's object id by then.
        let next = read_next_id(&add_path)?;
        dd::create::insert_objects(&add_path, &[], next + 1).map_err(|_| Error::CorruptTable)?;
        return Err(crate::ace_error::plain(not_valid(
            "An object with the same name already exists in the data dictionary.",
        )));
    }
    if super::rc4_tables::refuses_dbf_without_strong_encryption(&conn, table_type) {
        return Err(Error::Ace {
            code: 7168,
            detail: String::new(),
        });
    }
    let fields = source.fields();

    // The next object id is the dictionary's AutoInc next value (header 0x50).
    let mut next_id = read_next_id(&add_path)?;

    const DATABASE_ID: i32 = 1;
    let table_id = next_id;
    next_id += 1;

    // The Table object: its file path (with a leading ".\"), the "no auto
    // create" marker, and its field count.
    // A table inside the dictionary's folder is kept by its path from it,
    // however it was named (`fixtures/aes/rc4tp`, `y1`).
    let inside = Path::new(table_path)
        .strip_prefix(&folder)
        .ok()
        .map(|inside| inside.to_string_lossy().replace('/', "\\"));
    let relative = format!(".\\{}\0", inside.as_deref().unwrap_or(table_path));
    let field_count = (fields.len() as u16).to_le_bytes();
    let empty: &[u8] = &[];
    let table_slots: &[(usize, &[u8])] = &[
        (dd::table::RELATIVE_PATH, relative.as_bytes()),
        (dd::table::NO_AUTO_CREATE, empty),
        (dd::table::FIELD_COUNT, &field_count),
    ];
    let mut records = vec![dd::create::object_record(&NewObject {
        id: table_id,
        parent: DATABASE_ID,
        kind: "Table",
        name: table_name,
        info1: crate::dictionary::table_type_info(table_type),
        info2: crate::dictionary::table_info2(table_type, char_type),
        slots: table_slots,
    })];

    // A Field object per column. The engine writes them in reverse column
    // order, each carrying its real 1-based position in Info1.
    for (position, field) in fields.iter().enumerate().rev() {
        let definition = [
            field.field_type.code().to_le_bytes(),
            recorded_length(field).to_le_bytes(),
            field.decimals().to_le_bytes(),
        ]
        .concat();
        let options = dbf_field_options(field).to_le_bytes();
        let field_slots: &[(usize, &[u8])] = &[
            (dd::field::DEFINITION, &definition),
            (dd::field::OPTIONS, &options),
        ];
        records.push(dd::create::object_record(&NewObject {
            id: next_id,
            parent: table_id as i32,
            kind: "Field",
            name: &field.name,
            info1: position as i32 + 1,
            info2: 0x8000_0000,
            slots: field_slots,
        }));
        next_id += 1;
    }

    // An Index object per index file, with a Key object per tag.
    append_index_records(
        &source,
        table_id,
        &mut next_id,
        &folder,
        &dd::memo_path(&add_path),
        index_files,
        &mut records,
    )?;

    dd::create::insert_objects(&add_path, &records, next_id).map_err(|_| Error::CorruptTable)?;

    // Bind an ADT table's own file to the dictionary (its object id and the
    // dictionary's path), as the engine does; without it the engine opens the
    // table with error 7074. A DBF file has nowhere to say so.
    drop(source);
    let dictionary_file = add_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if table_type == TableType::Adt {
        dd::create::bind_table(&folder.join(table_path), table_id, &dictionary_file)
            .map_err(|_| Error::CorruptTable)?;
    }

    // Rebuild the dictionary's index over all its objects, the new ones among
    // them.
    let table = Arc::new(table::dictionary(session, &add_path)?);
    let weights = table
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = create::collation_crc(weights.as_ref());
    let ai_path = add_path.with_extension("ai");
    std::fs::remove_file(&ai_path).ok();
    index::build_dictionary_index(&table, &ai_path, collation)?;

    // The connection reloads its dictionary, so the new table is visible on it.
    conn.reload_database()?;
    // A dictionary of RC4 tables that encrypts new ones encrypts the table
    // added, its records, memo values and indexes
    // (`fixtures/aes/rc4tp`, `x2`).
    // One of AES tables does the same with a Visual FoxPro table, which it
    // lays out for AES first (`fixtures/aes/rc4tp`, `v5`).
    let aes_new = table_type == TableType::Vfp
        && conn
            .with_database_opt(|db| {
                db.scheme() != tflip_storage::crypto::Scheme::Rc4
                    && db
                        .dictionary()
                        .database()
                        .is_some_and(|d| d.has(dd::database::ENCRYPT_NEW_TABLE))
            })
            .unwrap_or(false);
    if super::rc4_tables::new_table_key(&conn).is_some() || aes_new {
        super::set_table_encryption(session, connection, table_name, true, false)?;
    }
    super::rights::grant_creator(session, connection, 1, table_name)
}

/// What the original says of an ADT table that is bound to a dictionary
/// already, or whose structure `connection` cannot read, when it is added.
fn check_not_bound(conn: &crate::connection::Connection, path: &Path) -> Result<(), Error> {
    use tflip_storage::dd::StructureCipher;
    let Ok(cipher) = tflip_storage::dd::create::structure_cipher(path) else {
        return Ok(());
    };
    let key = super::rc4_tables::table_key(conn);
    let rc4 = conn
        .with_database_opt(|db| db.scheme() == tflip_storage::crypto::Scheme::Rc4)
        .unwrap_or(false);
    let unreadable = match (cipher, &key) {
        (None, _) => None,
        (Some(StructureCipher::Rc4(_)), None) => Some(Error::PropertyNotSet),
        (Some(StructureCipher::Rc4(check)), Some(key)) => {
            (!tflip_storage::crypto::rc4_password_ok(&key[..key.len().min(20)], &check))
                .then(|| Error::InvalidPassword.detailed(path.display().to_string()))
        }
        (Some(StructureCipher::Aes), _) if rc4 && key.is_some() => {
            Some(Error::DictionaryEncrypted.detailed(path.display().to_string()))
        }
        (Some(StructureCipher::Aes), _) => None,
    };
    if let Some(error) = unreadable {
        return Err(error);
    }
    match tflip_storage::dd::create::is_bound(path).unwrap_or(false) {
        true => Err(Error::Ace {
            code: 5130,
            detail: "The table is not a free table.  It is already associated with a data \
                     dictionary."
                .to_owned(),
        }),
        false => Ok(()),
    }
}

/// The length a dictionary keeps an ADT field with: what the field stores,
/// which is more than it was declared for the variable-length and Unicode
/// types (`fixtures/sql/ddfieldlen`); a DBF field's is as declared.
pub(super) fn recorded_length(field: &tflip_storage::field::Field) -> u16 {
    use tflip_storage::field::FieldType::*;
    use tflip_storage::field::Layout;
    match (&field.layout, field.field_type) {
        (Layout::Adt, VarChar | VarBinary) => field.length() + 2,
        (Layout::Adt, NVarChar) => field.length() + 1,
        _ => field.length(),
    }
}

/// The options a dictionary keeps a field with: a VFP field's binary
/// (`NOCPTRANS`) and NULL flags, none for any other.
pub(super) fn dbf_field_options(field: &tflip_storage::field::Field) -> u32 {
    let flags = match &field.layout {
        tflip_storage::field::Layout::Dbf(layout) => layout.flags,
        _ => 0,
    };
    u32::from(flags & 0x04 != 0) | u32::from(flags & 0x02 != 0) << 1
}

/// The Key object `id` of the order `tag`, in the index file object
/// `index_file_id` of the table object `table_id` (`source` is the table, to
/// compile the key expression); `compound` is the `ADS_COMPOUND` bit of the
/// file's type. The dictionary records an index's options as the ADS_*
/// flags, which differ from the index file's own option bits.
pub(super) fn key_record(
    source: &table::Table,
    tag: &tflip_storage::order::Tag,
    compound: u32,
    table_id: u32,
    id: u32,
    index_file_id: u32,
    memo_path: &Path,
    fts: Option<&tflip_storage::create::FtsBlock>,
    noise: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    use tflip_storage::dd::{self, create::NewObject};
    const ADS_UNIQUE: u32 = 0x01;
    const ADS_CUSTOM: u32 = 0x04;
    const ADS_DESCENDING: u32 = 0x08;
    const ADS_CANDIDATE: u32 = 0x800;
    let expr_name = source.with_field_names(&tag.expression)?;
    // A full-text order's key is a word, whatever the field that holds the
    // text (a memo is no expression to compile).
    let key_type = match fts {
        Some(_) => 4,
        None => ads_key_type(source.compile(&expr_name)?.ty()),
    }
    .to_le_bytes();
    let expression = format!("{expr_name}\0");
    let condition = source.with_field_names(&tag.condition)?;
    let condition = (!condition.is_empty()).then(|| format!("{condition}\0"));
    let mut options = compound;
    if tag.unique() {
        options |= ADS_UNIQUE;
    }
    if tag.custom() {
        options |= ADS_CUSTOM;
    }
    if tag.flags & tflip_storage::order::flags::CANDIDATE != 0 {
        options |= ADS_CANDIDATE;
    }
    if tag.descending {
        options |= ADS_DESCENDING;
    }
    // The words' options are kept as the ADS_FTS_* flags (the full-text bit
    // among them), with the lists they split and drop by as texts of their
    // own (`fixtures/aes/dictwrite`, `43_fts_dictionary`).
    if let Some(block) = fts {
        options |= block.options | 0x20;
    }
    let options = options.to_le_bytes();
    let key_length = tag.key_length.to_le_bytes();
    let collation = format!("{}\0", tag.collation);
    let min_word = fts.map(|block| (block.min_word as u16).to_le_bytes());
    let lists = fts.map(|block| {
        [
            &block.delimiters[..],
            noise.unwrap_or(&block.noise[..]),
            &block.drop[..],
            &block.conditional[..],
        ]
        .map(|list| [list, &b"\0"[..]].concat())
    });
    let mut key_slots: Vec<(usize, &[u8])> = vec![
        (dd::index::EXPRESSION, expression.as_bytes()),
        (dd::index::OPTIONS, &options),
        (dd::index::KEY_LENGTH, &key_length),
        (dd::index::KEY_TYPE, &key_type),
        (dd::index::COLLATION, collation.as_bytes()),
    ];
    if let Some(condition) = &condition {
        key_slots.push((dd::index::CONDITION, condition.as_bytes()));
    }
    if let (Some(min_word), Some(lists)) = (&min_word, &lists) {
        key_slots.push((dd::index::FTS_MIN_LENGTH, min_word));
        for (slot, list) in [
            dd::index::FTS_DELIMITERS,
            dd::index::FTS_NOISE,
            dd::index::FTS_DROP_CHARS,
            dd::index::FTS_CONDITIONAL_CHARS,
        ]
        .into_iter()
        .zip(lists)
        {
            key_slots.push((slot, list));
        }
    }
    key_slots.sort_by_key(|(slot, _)| *slot);
    // The original sets an order's properties one at a time, the condition
    // and collation last, so an expression and condition that overflow the
    // record's property field continue in the memo file as a field's do
    // (`fixtures/sql/spddlongidx`).
    let order = [
        dd::index::EXPRESSION,
        dd::index::OPTIONS,
        dd::index::KEY_LENGTH,
        dd::index::KEY_TYPE,
        dd::index::FTS_MIN_LENGTH,
        dd::index::FTS_DELIMITERS,
        dd::index::FTS_DROP_CHARS,
        dd::index::FTS_CONDITIONAL_CHARS,
        dd::index::CONDITION,
        dd::index::COLLATION,
        dd::index::FTS_NOISE,
    ];
    dd::create::object_record_stepwise(
        &NewObject {
            id,
            parent: table_id as i32,
            kind: "Key",
            name: &tag.name,
            info1: index_file_id as i32,
            info2: 0x8000_0000,
            slots: &key_slots,
        },
        memo_path,
        &order,
    )
    .map_err(|_| Error::CorruptTable)
}

/// Appends an Index object and a Key object per order to `records` for each
/// file in `index_files` (`;`-separated, relative to `folder`), belonging to
/// the table object `table_id`; `source` is the table opened to compile the key
/// expressions. `next_id` advances past the new objects. Each index file's
/// records are as the original engine writes them (see [`add_table_api`]).
pub(super) fn append_index_records(
    source: &table::Table,
    table_id: u32,
    next_id: &mut u32,
    folder: &Path,
    memo_path: &Path,
    index_files: &str,
    records: &mut Vec<Vec<u8>>,
) -> Result<(), Error> {
    use tflip_storage::dbf::Flavour;
    use tflip_storage::dd::{self, create::NewObject};
    use tflip_storage::order::IndexFile;
    const ADS_COMPOUND: u32 = 0x02;
    for file_name in index_files
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let index_path = folder.join(file_name);
        if !index_path.exists() {
            return Err(Error::FileNotFound);
        }
        let mut index = IndexFile::open(&index_path).map_err(|_| Error::CorruptIndex)?;
        // An index file of another type than its table's is corrupt, as the
        // original engine says, each type its way
        // (`fixtures/dictionary/dbfcorners`).
        let refused = match (source.dbf_flavour(), &index) {
            (Some(Flavour::Clipper), IndexFile::Ntx(_)) => None,
            (Some(Flavour::FoxPro | Flavour::Visual), IndexFile::Cdx(_)) => None,
            (None, IndexFile::Adi(_)) => None,
            (Some(Flavour::Clipper), _) => Some((7015, "Corrupt NTX index.")),
            (Some(_), _) => Some((7017, "Corrupt .ADI, .CDX, or .IDX index.")),
            (None, _) => Some((7108, "The ADI version number is not recognized.")),
        };
        if let Some((code, text)) = refused {
            return Err(Error::Constraint {
                code,
                text: text.to_owned(),
            });
        }
        let tags = index.tags("");
        let index_file_id = *next_id;
        *next_id += 1;
        let index_relative = format!(".\\{file_name}\0");
        // An NTX file's one order is named after the file.
        let tags = match &index {
            IndexFile::Ntx(_) => {
                let stem = Path::new(file_name).file_stem().unwrap_or_default();
                index.tags(&stem.to_string_lossy())
            }
            _ => tags,
        };
        // The index file's type: ADS_NTX's 1, 3 for a CDX file, ADS_ADI's 6;
        // only the latter two are compound.
        let (index_type, compound) = match &index {
            IndexFile::Ntx(_) => (1, 0),
            IndexFile::Cdx(_) => (3, ADS_COMPOUND),
            IndexFile::Adi(_) => (6, ADS_COMPOUND),
        };
        let tag_count = (tags.len() as u16).to_le_bytes();
        let page_size = index.page_size().to_le_bytes();
        let index_slots: &[(usize, &[u8])] = &[
            (dd::index_file::RELATIVE_PATH, index_relative.as_bytes()),
            (1, &tag_count), // the number of orders the file holds
            (dd::index_file::PAGE_SIZE, &page_size),
        ];
        records.push(dd::create::object_record(&NewObject {
            id: index_file_id,
            parent: table_id as i32,
            kind: "Index",
            name: file_name,
            info1: index_type,
            info2: 0x8000_0000,
            slots: index_slots,
        }));

        for tag in &tags {
            let fts = index.fts_block(tag).map_err(|_| Error::CorruptIndex)?;
            records.push(key_record(
                source,
                tag,
                compound,
                table_id,
                *next_id,
                index_file_id,
                memo_path,
                fts.as_ref(),
                None,
            )?);
            *next_id += 1;
        }
    }
    Ok(())
}

/// Adds an index file to a table already in the dictionary
/// (`AdsDDAddIndexFile`): writes an Index object for `index_file_path` (relative
/// to the dictionary's folder) and a Key object per order, belonging to the
/// table `table_name`, then rebuilds the dictionary index. The index file must
/// exist. ADT index files only.
pub fn add_index_file_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    index_file_path: &str,
    _comment: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    use tflip_storage::table::Format;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "table", table_name)?;
    let table_id = table.id;
    super::rights::require(&conn, 1, table_name, super::rights::ALTER, "")?;
    // The file is looked for before anything is read: 7041 for one that is not
    // there, in a folder that is or not (`fixtures/sql/ddapirights2`).
    let mut index_file = folder.clone();
    index_file.extend(
        index_file_path
            .split(['\\', '/'])
            .filter(|part| !part.is_empty() && *part != "."),
    );
    if !index_file.is_file() {
        return Err(ace_error::sql(7041, ""));
    }

    // The table's own file, opened to compile the index key expressions.
    let table_file = table
        .text(dd::table::RELATIVE_PATH)
        .map(|p| folder.join(strip_relative(&p)))
        .ok_or(Error::CorruptTable)?;
    let source = Arc::new(
        super::rc4_tables::transient_in_dictionary(session, &conn, &table_file, Format::Adt)
            .map_err(|_| Error::FileNotFound)?,
    );

    let mut next_id = read_next_id(&add_path)?;
    let mut records = Vec::new();
    append_index_records(
        &source,
        table_id,
        &mut next_id,
        &folder,
        &dd::memo_path(&add_path),
        index_file_path,
        &mut records,
    )?;
    drop(source);
    dd::create::insert_objects(&add_path, &records, next_id).map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// Frees a table from the dictionary it belongs to (`AdsDDFreeTable`): clears
/// the dictionary binding in the table's own `.adt` file at `table_path` (given
/// the `.adt` extension if it has none) so it becomes a free table again.
/// `password` (for an encrypted table) is not used yet.
pub fn free_table_api(_session: &Session, table_path: &str, _password: &str) -> Result<(), Error> {
    use tflip_storage::dd;
    let mut path = PathBuf::from(table_path);
    if path.extension().is_none() {
        path.set_extension("adt");
    }
    if !path.exists() {
        return Err(Error::FileNotFound);
    }
    dd::create::free_table(&path).map_err(|_| Error::CorruptTable)
}

/// The dictionary's next object id (its AutoInc next value, header `0x50`).
pub(super) fn read_next_id(add_path: &Path) -> Result<u32, Error> {
    let mut header = [0u8; 0x54];
    std::fs::File::open(add_path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut header))
        .map_err(|_| Error::CorruptTable)?;
    Ok(u32::from_le_bytes(header[0x50..0x54].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::table::TableSettings;

    #[test]
    fn creates_a_dictionary_that_connects_and_reads() {
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddcreate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("new.add");
        let path_str = path.to_string_lossy().into_owned();

        let connect = create_dictionary_api(&session, &path_str, 0).unwrap();
        // The three files exist.
        assert!(path.exists(), ".add");
        assert!(dir.join("new.am").exists(), ".am");
        assert!(dir.join("new.ai").exists(), ".ai");

        // The connection is to a dictionary that reads back.
        let conn = connection::get(&session, connect).unwrap();
        assert!(conn.dictionary);
        assert!(conn.has_database());
        // Re-opening the written dictionary from disk works too.
        assert!(tflip_storage::dd::Dictionary::open(&path).is_ok());

        // Creating it again fails; the file is there (5128,
        // `fixtures/aes/dictwrite/14_ddcreate.json`).
        assert_eq!(
            create_dictionary_api(&session, &path_str, 0),
            Err(already_exists())
        );
        // An RC4 dictionary connects and reads as well (its files are
        // compared by `aes_replay`).
        let other = dir.join("enc.add");
        let encrypted = create_dictionary_api(&session, &other.to_string_lossy(), 1).unwrap();
        assert!(
            tflip_storage::dd::Dictionary::open(&other)
                .unwrap()
                .encrypted()
        );
        connection::disconnect(&session, encrypted).ok();

        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// What `AdsDDCreate101` makes of an encryption type and a dictionary
    /// password (`fixtures/aes/dictwrite/14_ddcreate.json`; the files are
    /// compared by `aes_replay`).
    #[test]
    fn creates_an_aes_dictionary_from_a_connect_string() {
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddcreate-aes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let create = |name: &str, options: &str| {
            let add = dir.join(name).with_extension("add");
            let source = format!("Data Source={};ServerType=LOCAL;{options}", add.display());
            create_dictionary_connect_api(&session, &source)
        };
        let code = |result: Result<Handle, Error>| result.unwrap_err().to_ace().code;

        let connect = create("a", "EncryptionType=aes128;DDPassword=ddpw;").unwrap();
        let conn = connection::get(&session, connect).unwrap();
        assert_eq!(
            conn.encryption_type(),
            tflip_storage::adt::ENCRYPTION_AES128
        );
        connection::disconnect(&session, connect).unwrap();
        let add = std::fs::read(dir.join("a.add")).unwrap();
        assert_eq!(add[0x190], tflip_storage::adt::ENCRYPTION_AES128);
        // RC4, or no type, makes a dictionary that is not AES.
        for options in ["EncryptionType=RC4;", "DDPassword=ddpw;"] {
            let connect = create("plain", options).unwrap();
            connection::disconnect(&session, connect).unwrap();
            let add = std::fs::read(dir.join("plain.add")).unwrap();
            assert_eq!(u32::from_le_bytes(add[0x170..0x174].try_into().unwrap()), 0);
            for file in ["plain.add", "plain.am", "plain.ai"] {
                std::fs::remove_file(dir.join(file)).unwrap();
            }
        }

        // The type is checked before anything is written.
        assert_eq!(code(create("b", "EncryptionType=;")), 5019);
        assert_eq!(
            code(create("b", "EncryptionType=AES999;DDPassword=x;")),
            5019
        );
        assert_eq!(
            code(create("b", "EncryptionType=AES128;DDPassword=;")),
            5019
        );
        assert!(!dir.join("b.add").exists());
        // A dictionary that is there is 5128, with or without a password.
        assert_eq!(code(create("a", "EncryptionType=AES128;")), 5128);
        // Without a password, or with more than 20 characters, the original
        // leaves the header and the memo file.
        assert_eq!(code(create("c", "EncryptionType=AES128;")), 5223);
        assert_eq!(std::fs::metadata(dir.join("c.add")).unwrap().len(), 2800);
        assert!(dir.join("c.am").exists() && !dir.join("c.ai").exists());
        assert_eq!(code(create("c", "EncryptionType=AES128;")), 5128);
        let long = "EncryptionType=AES256;DDPassword=123456789012345678901;";
        assert_eq!(code(create("d", long)), 5005);
        assert_eq!(std::fs::metadata(dir.join("d.add")).unwrap().len(), 2800);
        let twenty = "EncryptionType=AES256;DDPassword=12345678901234567890;";
        assert!(create("e", twenty).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn adds_a_table_to_a_dictionary_like_the_engine() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/new");
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddaddtable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        // The table to add, beside the dictionary (the engine's own t.adt).
        copy_free(&fixtures.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();

        let ours = std::fs::read(&add).unwrap();
        let golden = std::fs::read(fixtures.join("one_table.add")).unwrap();
        assert_eq!(ours.len(), golden.len(), "add length");
        // Record count is 4 (Database, Table, two Fields).
        assert_eq!(u32::from_le_bytes(ours[0x18..0x1c].try_into().unwrap()), 4);
        assert_eq!(
            u32::from_le_bytes(ours[0x50..0x54].try_into().unwrap()),
            5,
            "next id"
        );
        // The three new records (Table and two Fields) match the engine byte
        // for byte; record 1's login secret is random, so it is left aside.
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let rl = 524;
        assert_eq!(
            &ours[data + rl..data + 4 * rl],
            &golden[data + rl..data + 4 * rl],
            "new records"
        );
        // The rebuilt index matches the engine's.
        let our_ai = std::fs::read(dir.join("db.ai")).unwrap();
        let golden_ai = std::fs::read(fixtures.join("one_table.ai")).unwrap();
        assert_eq!(our_ai, golden_ai, "index");

        // The table's own file is bound to the dictionary as the engine binds
        // it (its object id at 0x14 and 0x160, the dictionary's path at 0x5c),
        // so the original engine opens it. The engine's t.adt fixture is
        // already bound, so a byte-for-byte match confirms it.
        assert_eq!(
            std::fs::read(dir.join("t.adt")).unwrap(),
            std::fs::read(fixtures.join("t.adt")).unwrap(),
            "the table file is bound to the dictionary"
        );

        // The new table is visible on the same connection: it opens by name.
        let opened = table::open(&session, connect, "t", TableSettings::default()).unwrap();
        assert!(table::get(&session, opened).is_ok());

        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// DBF tables join a dictionary as the original engine adds them
    /// (`fixtures/dictionary/dbftables`, made by `tools/ace/dd/ddwrite.c`): a
    /// FoxPro table with a memo and a compound index, a Clipper table with an
    /// NTX index, a Visual FoxPro table with a compound index (its Integer
    /// field flagged binary) and a FoxPro table without an index, their files
    /// left as they were; they open by name as their own types.
    #[test]
    fn adds_dbf_tables_like_the_engine() {
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/dbftables");
        let session = Session::new();
        let dir =
            tflip_storage::testing::temp_root().join(format!("tflip-dddbf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(fixtures.join("free")).unwrap() {
            let path = entry.unwrap().path();
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        for (name, file, table_type, index) in [
            ("c", "c.dbf", TableType::Cdx, "c.cdx"),
            ("n", "n.dbf", TableType::Ntx, "n_id.ntx"),
            ("v", "v.dbf", TableType::Vfp, "v.cdx"),
            ("c0", "c0.dbf", TableType::Cdx, ""),
        ] {
            add_table_api(&session, connect, name, file, table_type, 1, index, "").unwrap();
        }

        let ours = std::fs::read(&add).unwrap();
        let golden = std::fs::read(fixtures.join("db.add")).unwrap();
        assert_eq!(ours.len(), golden.len(), "add length");
        // Every record but the Database's (its login secret is random).
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let rl = 524;
        for record in 1..20 {
            let at = data + record * rl;
            assert_eq!(
                &ours[at..at + rl],
                &golden[at..at + rl],
                "record {}",
                record + 1
            );
        }
        for entry in std::fs::read_dir(&fixtures).unwrap() {
            let path = entry.unwrap().path();
            // The memo and index files and the tables, left as they were.
            if path.is_file() && path.extension().is_none_or(|e| e != "add") {
                let name = path.file_name().unwrap();
                let ours = std::fs::read(dir.join(name)).unwrap();
                assert!(ours == std::fs::read(&path).unwrap(), "{}", name.display());
            }
        }

        // Each opens by name as its own type, with its index file.
        for (name, table_type, count) in [
            ("c", TableType::Cdx, 1),
            ("n", TableType::Ntx, 1),
            ("v", TableType::Vfp, 1),
            ("c0", TableType::Cdx, 0),
        ] {
            let opened = table::open(&session, connect, name, TableSettings::default()).unwrap();
            let table = table::get(&session, opened).unwrap();
            assert_eq!(table.settings.table_type, table_type, "{name}");
            assert_eq!(table.record_count().unwrap(), 2, "{name}");
            assert_eq!(
                crate::index::orders_of(&session, opened).unwrap().len(),
                count,
                "{name}"
            );
            table::close(&session, opened).unwrap();
        }

        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frees_a_table_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddfree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A bound table (the engine's, as AddTable binds it), then free it.
        let t = dir.join("t.adt");
        copy_free(&fx.join("t.adt"), &t);
        free_table_api(&session, &t.to_string_lossy(), "").unwrap();
        assert_eq!(
            std::fs::read(&t).unwrap(),
            std::fs::read(fx.join("freed_t.adt")).unwrap(),
            "the table is unbound like the engine"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adds_an_index_file_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddaddidxf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t (no index), then add the index file ix.adi separately.
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        std::fs::copy(fx.join("ix.adi"), dir.join("ix.adi")).unwrap();
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        add_index_file_api(&session, connect, "t", "ix.adi", "").unwrap();

        let ours = std::fs::read(&add).unwrap();
        let golden = std::fs::read(fx.join("addindexfile.add")).unwrap();
        assert_eq!(mask_login(ours.clone()), mask_login(golden), ".add");
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("addindexfile.ai")).unwrap(),
            ".ai"
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adds_a_table_with_an_index_like_the_engine() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/new");
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddaddidx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        // The engine's own table and index file, as "t".
        copy_free(&fixtures.join("it.adt"), &dir.join("t.adt"));
        std::fs::copy(fixtures.join("it.adi"), dir.join("t.adi")).unwrap();
        add_table_api(
            &session,
            connect,
            "t",
            "t.adt",
            TableType::Adt,
            1,
            "t.adi",
            "",
        )
        .unwrap();

        let ours = std::fs::read(&add).unwrap();
        let golden = std::fs::read(fixtures.join("indexed_table.add")).unwrap();
        assert_eq!(ours.len(), golden.len(), "add length");
        // 7 records: Database, Table, two Fields, an Index and two Keys.
        assert_eq!(u32::from_le_bytes(ours[0x18..0x1c].try_into().unwrap()), 7);
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let rl = 524;
        assert_eq!(
            &ours[data + rl..data + 7 * rl],
            &golden[data + rl..data + 7 * rl],
            "the table, field, index and key records"
        );
        let our_ai = std::fs::read(dir.join("db.ai")).unwrap();
        assert_eq!(
            our_ai,
            std::fs::read(fixtures.join("indexed_table.ai")).unwrap(),
            "index"
        );

        // The table file is bound to the dictionary (the engine's it.adt is).
        assert_eq!(
            std::fs::read(dir.join("t.adt")).unwrap(),
            std::fs::read(fixtures.join("it.adt")).unwrap(),
            "the table file is bound to the dictionary"
        );

        // A table that is not there is not added (its path is in the
        // error); the table type the dictionary decides is no type to add.
        assert_eq!(
            add_table_api(&session, connect, "u", "u.dbf", TableType::Cdx, 1, "", ""),
            Err(Error::FileNotFound.detailed(dir.join("u.dbf").display().to_string()))
        );
        assert_eq!(
            add_table_api(
                &session,
                connect,
                "u",
                "t.adt",
                TableType::Database,
                1,
                "",
                ""
            ),
            Err(Error::InvalidTableType)
        );

        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }
}
