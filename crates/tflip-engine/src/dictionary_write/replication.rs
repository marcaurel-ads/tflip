//! Publications, articles, subscriptions and their queue files.

use super::*;

/// Creates a replication publication in the data dictionary `connection` is to
/// (`AdsDDCreatePublication`): a `Publicatn` object under the Database, holding
/// the publication options (slot 0, always written) and the comment (if any) in
/// the dictionary memo — written as the original engine writes it, then the
/// index is rebuilt and the connection reloaded. The create options must be 0
/// (the engine refuses others with 5019); other options are set afterwards with
/// [`set_publication_property_api`]. An empty or already-used name is refused
/// with 5132.
pub fn create_publication_api(
    session: &Session,
    connection: Handle,
    name: &str,
    comment: &str,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::rights::require_create(&conn, 15, "publication")?;
    if name.is_empty() {
        return Err(invalid_object_name(name));
    }
    let dict = open_dict(&add_path)?;
    if dict.named(Kind::Publication, name).is_some() {
        return Err(invalid_object_name(name));
    }
    drop(dict);

    const DATABASE_ID: i32 = 1;
    let next_id = read_next_id(&add_path)?;
    let options = options.to_le_bytes();
    let slots: &[(usize, &[u8])] = &[(dd::publication::OPTIONS, &options)];
    let mut record = dd::create::object_record(&NewObject {
        id: next_id,
        parent: DATABASE_ID,
        kind: "Publicatn",
        name,
        info1: i32::MIN, // 0x80000000, NULL as the engine writes it
        info2: 0x8000_0000,
        slots,
    });
    // AdsDDCreatePublication stores the comment's length without its NUL
    // terminator (the memo block is zero-filled, so a reader still finds one).
    dd::create::set_comment_strlen(&mut record, &dd::memo_path(&add_path), comment.as_bytes())
        .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)?;
    super::rights::grant_creator(session, connection, 15, name)
}

/// What a user who may not alter a publication is told when it changes the
/// publication or its articles (`fixtures/sql/ddapirights`).
const ALTER_PUBLICATION: &str = "Permission to alter the publication is not granted.";

/// Deletes a replication publication from the data dictionary
/// (`AdsDDDeletePublication`): soft-deletes its `Publicatn` object and every
/// `Article` under it, freeing the publication's comment memo, as the original
/// engine does. A name that is no publication fails with 5132.
pub fn delete_publication_api(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let Some(publication) = dict.named(Kind::Publication, name) else {
        return Err(not_valid("The publication is not part of the database."));
    };
    super::rights::require(
        &conn,
        15,
        name,
        super::rights::DROP,
        "Permission to drop the publication is not granted.",
    )?;
    if dict
        .all(Kind::Subscription)
        .iter()
        .any(|s| s.info1 == Some(publication.id as i32))
    {
        return Err(not_valid(
            "The publication is used by one or more subscriptions and cannot be deleted.",
        ));
    }
    // The publication goes first, then the rights on it, then the articles
    // under it, highest record first so the free list threads as the
    // engine's does (`fixtures/sql/ddapirights`).
    let mut records = vec![publication.record()];
    records.extend(permission_records(&dict, publication.id));
    let mut articles: Vec<u32> = dict
        .children(publication.id, Kind::Article)
        .iter()
        .map(|a| a.record())
        .collect();
    articles.sort_unstable_by(|a, b| b.cmp(a));
    records.extend(articles);
    drop(dict);

    delete_records(session, &conn, &add_path, &records)
}

/// Sets a property of a replication publication (`AdsDDSetPublicationProperty`):
/// `ADS_DD_COMMENT` goes to the publication's comment, `ADS_DD_PUBLICATION_OPTIONS`
/// and `ADS_DD_USER_DEFINED_PROP` to its property blob (verbatim, a
/// read-modify-write keeping the other slots), as the original engine stores
/// them. A name that is no publication fails with 5132; other properties are
/// [`Error::NotImplemented`].
pub fn set_publication_property_api(
    session: &Session,
    connection: Handle,
    name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::properties::require_known_property(
        property_id,
        super::properties::known_properties::PUBLICATION,
        "a publication object",
    )?;
    let dict = open_dict(&add_path)?;
    let publication = find_named_api(&dict, Kind::Publication, "publication", name)?;
    super::rights::require(&conn, 15, name, super::rights::ALTER, ALTER_PUBLICATION)?;
    let slot = publication.record();
    drop(dict);
    let index = match property_id {
        ADS_DD_COMMENT => {
            return set_replication_comment(session, &conn, &add_path, slot, comment_value(value));
        }
        1500 => dd::publication::OPTIONS, // ADS_DD_PUBLICATION_OPTIONS (u32)
        ADS_DD_USER_DEFINED_PROP => dd::publication::USER_DEFINED,
        _ => return Err(Error::NotImplemented),
    };
    set_object_slot(session, &conn, &add_path, slot, index, value)
}

/// `ADS_IDENTIFY_BY_PRIMARY`: an article's row-identifying columns are the
/// published table's primary key.
const ADS_IDENTIFY_BY_PRIMARY: u32 = 0x01;

/// `ADS_IDENTIFY_BY_ALL`: an article's row-identifying columns are all of them.
const ADS_IDENTIFY_BY_ALL: u32 = 0x02;

/// A column list as an article stores it: each column followed by `;` (the
/// engine's form, whatever separators — `;` or `,` — the caller used).
fn article_columns<'a>(columns: impl IntoIterator<Item = &'a str>) -> String {
    columns
        .into_iter()
        .flat_map(|c| c.split([';', ',']))
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .fold(String::new(), |mut s, c| {
            s.push_str(c);
            s.push(';');
            s
        })
}

/// The filter with each field name rewritten to `F<field number>`, as an article
/// keeps its "short" filter: a lexical pass that replaces identifier tokens that
/// name a field and leaves string literals, numbers and operators as they are.
fn article_filter_short(
    filter: &str,
    field_number: &std::collections::HashMap<String, i32>,
) -> String {
    let bytes = filter.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' {
            // A string literal: copied verbatim, up to and including its close.
            out.push(c as char);
            i += 1;
            while i < bytes.len() {
                out.push(bytes[i] as char);
                let close = bytes[i] == c;
                i += 1;
                if close {
                    break;
                }
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &filter[start..i];
            match field_number.get(&ident.to_ascii_lowercase()) {
                Some(number) => {
                    out.push('F');
                    out.push_str(&number.to_string());
                }
                None => out.push_str(ident),
            }
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

/// 5192, the row-identification columns an article asks for are not valid;
/// `detail` is what the engine adds to the code's text (probed only for none
/// asked for, `fixtures/sql/ddrepl`).
fn invalid_replication_ident(detail: &str) -> Error {
    ace_error::sql(5192, detail)
}

/// Creates a replication article in the data dictionary (`AdsDDCreateArticle`,
/// `AdsDDCreateArticle100`): an `Article` object under its publication, named by
/// the published table's object id as 8 hex digits (its `Info1` that id), holding
/// the row filter (slot 0), the row-identifying columns (slot 1) and the filter
/// with field names rewritten to `F<n>` (slot 2) — written as the original
/// engine writes it. The identifying columns come from `row_ident_columns`, or,
/// when it is empty, from `options` (`ADS_IDENTIFY_BY_ALL` or `_BY_PRIMARY`). An
/// unknown publication or table, or no valid identifying columns, is refused.
#[allow(clippy::too_many_arguments)]
pub fn create_article_api(
    session: &Session,
    connection: Handle,
    publication: &str,
    table_name: &str,
    row_ident_columns: &str,
    filter: &str,
    options: u32,
    wide: bool,
) -> Result<(), Error> {
    use std::collections::HashMap;

    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let pub_object = find_named_api(&dict, Kind::Publication, "publication", publication)?;
    let pub_id = pub_object.id;
    let table = find_named_api(&dict, Kind::Table, "object", table_name)?;
    let table_id = table.id;
    let name = format!("{table_id:08X}");
    if dict.child(pub_id, Kind::Article, &name).is_some() {
        return Err(not_valid("Article already exists in the publication."));
    }
    // The table's fields, by name (lower case) to number and in number order.
    let mut fields: Vec<(&str, i32)> = dict
        .children(table_id, Kind::Field)
        .iter()
        .filter_map(|f| f.info1.map(|n| (f.name.as_str(), n)))
        .collect();
    fields.sort_by_key(|&(_, n)| n);
    let field_number: HashMap<String, i32> = fields
        .iter()
        .map(|&(n, i)| (n.to_ascii_lowercase(), i))
        .collect();

    // The row-identifying columns: as given, or all, or the primary key.
    let id_columns = if !row_ident_columns.is_empty() {
        article_columns([row_ident_columns])
    } else if options & ADS_IDENTIFY_BY_ALL != 0 {
        article_columns(fields.iter().map(|&(n, _)| n))
    } else if options & ADS_IDENTIFY_BY_PRIMARY != 0 {
        let key = table
            .info1
            .filter(|&id| id > 0)
            .and_then(|id| dict.object(id as u32))
            .and_then(|k| k.text(dd::index::EXPRESSION))
            .ok_or_else(|| invalid_replication_ident(""))?;
        // The key's field names, in order, as the identifying columns.
        let idents: Vec<String> = article_filter_short(&key, &HashMap::new())
            .split(';')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        article_columns(idents.iter().map(String::as_str))
    } else {
        return Err(invalid_replication_ident("No columns were specified."));
    };
    if id_columns.is_empty() {
        return Err(invalid_replication_ident(""));
    }
    let short = article_filter_short(filter, &field_number);
    drop(dict);

    let next_id = read_next_id(&add_path)?;
    let filter_nul = nul_bytes(filter);
    // AdsDDCreateArticle100 (a wide filter) marks the short filter as UTF-8 with
    // a byte-order mark; the readable filter stays narrow.
    let short_nul = if wide {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(short.as_bytes());
        bytes.push(0);
        bytes
    } else {
        nul_bytes(&short)
    };
    let id_columns_nul = nul_bytes(&id_columns);
    let mut slots: Vec<(usize, &[u8])> = Vec::new();
    if !filter.is_empty() {
        slots.push((dd::article::FILTER, &filter_nul));
        slots.push((dd::article::FILTER_SHORT, &short_nul));
    }
    slots.push((dd::article::ID_COLUMNS, &id_columns_nul));
    slots.sort_by_key(|&(i, _)| i);
    let record = dd::create::object_record(&NewObject {
        id: next_id,
        parent: pub_id as i32,
        kind: "Article",
        name: &name,
        info1: table_id as i32,
        info2: 0x8000_0000,
        slots: &slots,
    });
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// Deletes a replication article (`AdsDDDeleteArticle`): soft-deletes the
/// `Article` object for `table_name` under `publication`. An unknown publication,
/// table or article is refused.
pub fn delete_article_api(
    session: &Session,
    connection: Handle,
    publication: &str,
    table_name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let Some(pub_object) = dict.named(Kind::Publication, publication) else {
        return Err(not_valid("The publication is not part of the database."));
    };
    super::rights::require(
        &conn,
        15,
        publication,
        super::rights::ALTER,
        ALTER_PUBLICATION,
    )?;
    let Some(table) = dict.named(Kind::Table, table_name) else {
        return Err(not_valid(
            "The table could not be found in the data dictionary.",
        ));
    };
    let name = format!("{:08X}", table.id);
    let article = dict
        .child(pub_object.id, Kind::Article, &name)
        .ok_or_else(|| no_such_object("article", table_name))?;
    let record = article.record();
    drop(dict);
    delete_records(session, &conn, &add_path, &[record])
}

/// Sets a property of a replication article (`AdsDDSetArticleProperty`): the
/// include or exclude column list (slot 3, with the kind flag in slot 4), the
/// insert- and update-merge flags (present-empty slots 5 and 6), as the original
/// engine stores them. An unknown publication, table or article is refused;
/// other properties are [`Error::NotImplemented`].
pub fn set_article_property_api(
    session: &Session,
    connection: Handle,
    publication: &str,
    table_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::properties::require_known_property(
        property_id,
        super::properties::known_properties::ARTICLE,
        "an article object",
    )?;
    let dict = open_dict(&add_path)?;
    let pub_object = find_named_api(&dict, Kind::Publication, "publication", publication)?;
    super::rights::require(
        &conn,
        15,
        publication,
        super::rights::ALTER,
        ALTER_PUBLICATION,
    )?;
    let Some(table) = dict.named(Kind::Table, table_name) else {
        return Err(not_valid(
            "The table could not be found in the data dictionary.",
        ));
    };
    let name = format!("{:08X}", table.id);
    let article = dict
        .child(pub_object.id, Kind::Article, &name)
        .ok_or_else(|| no_such_object("article", table_name))?;
    let slot = article.record();
    drop(dict);
    // The include and exclude lists share slot 3; slot 4 says which it is.
    let list_kind = |kind: u16| kind.to_le_bytes();
    match property_id {
        // ADS_DD_ARTICLE_INCLUDE_COLUMNS / _EXCLUDE_COLUMNS.
        1604 | 1605 => {
            let kind = if property_id == 1604 {
                dd::article::LIST_INCLUDE
            } else {
                dd::article::LIST_EXCLUDE
            };
            let text = String::from_utf8_lossy(comment_value(value)).into_owned();
            let columns = nul_bytes(&article_columns([text.as_str()]));
            let flag = list_kind(kind);
            let slots: &[(usize, &[u8])] = &[
                (dd::article::COLUMN_LIST, &columns),
                (dd::article::LIST_KIND, &flag),
            ];
            set_object_slots(session, &conn, &add_path, slot, slots)
        }
        // ADS_DD_ARTICLE_INSERT_MERGE / _UPDATE_MERGE: present-empty flag slots.
        1607 | 1608 => {
            let index = if property_id == 1607 {
                dd::article::INSERT_MERGE
            } else {
                dd::article::UPDATE_MERGE
            };
            set_object_presence(
                session,
                &conn,
                &add_path,
                slot,
                index,
                value_u16(value) != 0,
            )
        }
        _ => Err(Error::NotImplemented),
    }
}

/// The replication queue table's 17 fields, in the order the engine creates
/// their dictionary objects (highest field number first): name, field number,
/// `ADS_*` type code and length.
const QUEUE_FIELDS: &[(&str, u16, u16, u16)] = &[
    ("ErrorInfo2", 17, 5, 9),
    ("ErrorInfo", 16, 5, 9),
    ("ErrorCode", 15, 11, 4),
    ("BlobData", 14, 6, 9),
    ("RecordData", 13, 6, 9),
    ("UpdateType", 12, 4, 1),
    ("RecordNumber", 11, 11, 4),
    ("IdentColumns", 10, 6, 9),
    ("CompatLock", 9, 1, 1),
    ("CRC", 8, 11, 4),
    ("TableName", 7, 8, 30),
    ("AllDBID", 6, 6, 9),
    ("ThisDBID", 5, 11, 4),
    ("TxnReady", 4, 11, 4),
    ("TxnID", 3, 11, 4),
    ("EntryID", 2, 15, 4),
    ("UpdateTime", 1, 14, 8),
];

/// The record-number offsets from the queue table, in the order the engine
/// soft-deletes the queue objects when a subscription is dropped (the subscription
/// record itself goes last). Stable across dictionaries (verified against the
/// engine); it threads the deleted-record free list as the engine's does.
const QUEUE_DELETE_ORDER: &[u32] = &[
    0, 12, 4, 9, 10, 16, 3, 2, 1, 8, 5, 7, 11, 13, 15, 14, 17, 6, 18, 20, 19,
];

/// The replication queue table's dictionary objects, in record order: the
/// `__<name>` Table (bound to `queue_file`), its 17 fields, its index file
/// `index_file` and the `TXNREADY` and `TXNENTRY` index orders — as the original
/// engine writes them when a subscription is created. `table_id` is the table's
/// object id (its fields follow at `table_id + 1`, ...).
fn queue_table_objects(table_id: u32, sub_name: &str, location: &QueueLocation) -> Vec<Vec<u8>> {
    use tflip_storage::dd::{self, create::NewObject, create::object_record};
    const NULL: u32 = 0x8000_0000;
    let mut records = Vec::new();

    // The table, bound to the queue file, with a memo block size of 64 and a
    // default index of TxnReady.
    let name = format!("__{sub_name}");
    let rel = nul_bytes(&location.table_path);
    let field_count = 17u16.to_le_bytes();
    let memo_size = 64u16.to_le_bytes();
    let default_index = nul_bytes("TxnReady");
    let table_slots: Vec<(usize, &[u8])> = vec![
        (dd::table::RELATIVE_PATH, &rel),
        (dd::table::FIELD_COUNT, &field_count),
        (dd::table::MEMO_BLOCK_SIZE, &memo_size),
        (dd::table::DEFAULT_INDEX, &default_index),
    ];
    records.push(object_record(&NewObject {
        id: table_id,
        parent: 1,
        kind: "Table",
        name: &name,
        info1: 0,
        info2: 0,
        slots: &table_slots,
    }));

    // The fields.
    let now = nul_bytes("now()");
    for (i, &(field_name, number, ty, len)) in QUEUE_FIELDS.iter().enumerate() {
        let id = table_id + 1 + i as u32;
        let mut def = Vec::with_capacity(6);
        def.extend_from_slice(&ty.to_le_bytes());
        def.extend_from_slice(&len.to_le_bytes());
        def.extend_from_slice(&0u16.to_le_bytes());
        let options = 0u32.to_le_bytes();
        let mut slots: Vec<(usize, &[u8])> = vec![
            (dd::field::DEFINITION, &def),
            (dd::field::OPTIONS, &options),
        ];
        if field_name == "UpdateTime" {
            slots.insert(0, (dd::field::DEFAULT_VALUE, &now));
        }
        records.push(object_record(&NewObject {
            id,
            parent: table_id as i32,
            kind: "Field",
            name: field_name,
            info1: i32::from(number),
            info2: NULL,
            slots: &slots,
        }));
    }

    // The index file and its two orders.
    let index_id = table_id + 18;
    let index_rel = nul_bytes(&location.index_path);
    let in_file = 1u16.to_le_bytes();
    let page_size = 512u32.to_le_bytes();
    let index_slots: Vec<(usize, &[u8])> = vec![
        (dd::index_file::RELATIVE_PATH, &index_rel),
        (1, &in_file),
        (dd::index_file::PAGE_SIZE, &page_size),
    ];
    records.push(object_record(&NewObject {
        id: index_id,
        parent: table_id as i32,
        kind: "Index",
        name: &location.index_name,
        info1: 6,
        info2: NULL,
        slots: &index_slots,
    }));

    let options = 2u32.to_le_bytes();
    let collation = [0u8];
    for (order_name, expr, cond, key_len, key_type) in [
        ("TXNREADY", "TxnReady", "TxnReady <> NULL", 8u16, 2u16),
        ("TXNENTRY", "TxnID;EntryID", "TxnID <> NULL", 16u16, 16u16),
    ] {
        let id = if order_name == "TXNREADY" {
            table_id + 19
        } else {
            table_id + 20
        };
        let expr = nul_bytes(expr);
        let cond = nul_bytes(cond);
        let key_len = key_len.to_le_bytes();
        let key_type = key_type.to_le_bytes();
        let slots: Vec<(usize, &[u8])> = vec![
            (dd::index::EXPRESSION, &expr),
            (dd::index::CONDITION, &cond),
            (dd::index::OPTIONS, &options),
            (dd::index::KEY_LENGTH, &key_len),
            (dd::index::KEY_TYPE, &key_type),
            (dd::index::COLLATION, &collation),
        ];
        records.push(object_record(&NewObject {
            id,
            parent: table_id as i32,
            kind: "Key",
            name: order_name,
            info1: index_id as i32,
            info2: NULL,
            slots: &slots,
        }));
    }
    records
}

/// The queue file's companion names, from the queue file (its data file): the
/// base without extension, and the memo (`.adm`) and index (`.adi`) files.
fn queue_companions(queue_file: &str) -> (String, String) {
    let base = queue_file
        .rsplit_once('.')
        .map_or(queue_file, |(stem, _)| stem);
    (format!("{base}.adm"), format!("{base}.adi"))
}

/// Where a subscription's queue table and its index file are
/// (`fixtures/sql/ddrepl`).
struct QueueLocation {
    /// The table's and the index file's paths as the dictionary keeps them.
    table_path: String,
    index_path: String,
    /// The index file's name, which its dictionary object has.
    index_name: String,
    /// The queue's three files in the dictionary's folder, which the engine
    /// makes only for a name without a folder.
    files: Option<[String; 3]>,
}

/// The place of the queue named `queue`: a name without an extension gets
/// `.adt`, and one that ends in a separator or a dot is no file (5127). For a
/// name with a folder in it the engine keeps paths of its own making (a `..`
/// in front of the folder, relative to `.`) and makes no files.
fn queue_location(queue: &str) -> Result<QueueLocation, Error> {
    let is_separator = |c: char| c == '\\' || c == '/';
    if queue.is_empty() || queue.ends_with(is_separator) || queue.ends_with('.') {
        return Err(ace_error::sql(
            5127,
            "The replication queue must be a file.",
        ));
    }
    let file_part = queue.rsplit(is_separator).next().unwrap_or(queue);
    let data = match file_part.contains('.') {
        true => queue.to_owned(),
        false => format!("{queue}.adt"),
    };
    let (memo, mut index) = queue_companions(&data);
    // A name with two extensions (`q.x.y`) has the first as its index file,
    // whole, where one with a single extension gets `.adi`.
    if data
        .rsplit(is_separator)
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .is_some_and(|(base, _)| base.contains('.'))
    {
        index = memo.trim_end_matches(".adm").to_owned();
    }
    let in_folder = queue.contains(is_separator);
    let path_of = |file: &str| {
        let file = file.replace('/', "\\");
        match in_folder {
            false => format!(".\\{file}"),
            true if file.starts_with('.') => format!("..{file}"),
            true => format!("...\\{file}"),
        }
    };
    Ok(QueueLocation {
        table_path: path_of(&data),
        index_path: path_of(&index),
        index_name: index
            .rsplit(is_separator)
            .next()
            .unwrap_or(&index)
            .to_owned(),
        files: (!in_folder).then(|| [data, memo, index]),
    })
}

/// Writes the replication queue table's three files (`.add` data, `.adm` memo,
/// `.adi` index) in `folder` from the fixed templates, patching the data file's
/// bound object id (`0x160`) and its dictionary back-reference (`0x5c`), as the
/// original engine writes them when a subscription is created.
fn write_queue_files(
    folder: &Path,
    dict_file: &str,
    queue_file: &str,
    memo_file: &str,
    index_file: &str,
    table_id: u32,
) -> Result<(), Error> {
    let mut data = include_bytes!("../replication/queue.add").to_vec();
    data[0x160..0x164].copy_from_slice(&table_id.to_le_bytes());
    // The back-reference path field runs from 0x5c to 0x160.
    let back_reference = format!(".\\{dict_file}\0");
    let field = &mut data[0x5C..0x160];
    field.fill(0);
    field[..back_reference.len()].copy_from_slice(back_reference.as_bytes());
    std::fs::write(folder.join(queue_file), &data).map_err(|_| Error::CorruptTable)?;
    std::fs::write(
        folder.join(memo_file),
        include_bytes!("../replication/queue.adm"),
    )
    .map_err(|_| Error::CorruptTable)?;
    std::fs::write(
        folder.join(index_file),
        include_bytes!("../replication/queue.adi"),
    )
    .map_err(|_| Error::CorruptTable)?;
    Ok(())
}

/// Makes the files of a queue table a deployment brings to the folder, as
/// they are when the subscription is made: from the paths the dictionary keeps
/// for the table and its index file, bound to the table's new object id. A
/// queue with a folder in its path has none (`fixtures/dictionary/deploy/x_sub*`).
pub(super) fn make_deployed_queue(
    folder: &Path,
    dictionary_file: &str,
    table_path: &str,
    index_path: &str,
    table_id: u32,
) -> Result<(), Error> {
    let own = |path: &str| {
        path.strip_prefix(".\\")
            .filter(|file| !file.contains(['\\', '/']))
            .map(str::to_owned)
    };
    let (Some(data), Some(index)) = (own(table_path), own(index_path)) else {
        return Ok(());
    };
    let (memo, _) = queue_companions(&data);
    write_queue_files(folder, dictionary_file, &data, &memo, &index, table_id)
}

/// Creates a replication subscription (`AdsDDCreateSubscription`): a `Subscriptn`
/// object under the Database (its `Info1` the publication's object id) holding the
/// target, user name, enciphered password, the forward/enabled/paused flags, the
/// replication queue and the options; and the replication queue table it needs —
/// a `__<name>` table of a fixed schema bound to the queue file, with its fields,
/// index and two orders, and the three queue files on disk — all as the original
/// engine writes them. The password is enciphered with the dictionary key. An
/// empty or used name, or an unknown publication, is refused.
#[allow(clippy::too_many_arguments)]
pub fn create_subscription_api(
    session: &Session,
    connection: Handle,
    name: &str,
    publication: &str,
    target: &str,
    user: &str,
    password: &str,
    queue: &str,
    forward: u16,
    comment: &str,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if name.is_empty() {
        return Err(invalid_object_name(name));
    }
    let dict = open_dict(&add_path)?;
    if dict.named(Kind::Subscription, name).is_some() {
        return Err(not_valid(
            "Subscription already exists in the dictionary with the same name.",
        ));
    }
    let publication = dict
        .named(Kind::Publication, publication)
        .ok_or_else(|| not_valid("The publication does not exist."))?;
    let pub_id = publication.id;
    // Before anything is written: the comment's memo block is taken with the
    // subscription's record. The engine takes the subscription's object id
    // before it looks at the queue, and keeps it spent.
    let location = match queue_location(queue) {
        Ok(location) => location,
        Err(error) => {
            let spent = read_next_id(&add_path)?;
            dd::create::insert_objects(&add_path, &[], spent + 1)
                .map_err(|_| Error::CorruptTable)?;
            return Err(error);
        }
    };
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
    let scheme = dict.scheme();
    drop(dict);

    let sub_id = read_next_id(&add_path)?;
    let table_id = sub_id + 1;
    // The subscription object.
    let target_nul = nul_bytes(target);
    let user_nul = nul_bytes(user);
    let password_enc = tflip_storage::crypto::encipher_subscription_secret(
        scheme,
        &key,
        sub_id,
        dd::subscription::PASSWORD as u32,
        password.as_bytes(),
    );
    let forward_bytes = forward.to_le_bytes();
    let enabled = 1u16.to_le_bytes();
    let queue_nul = nul_bytes(queue);
    let options_bytes = options.to_le_bytes();
    let paused = 0u16.to_le_bytes();
    let slots: Vec<(usize, &[u8])> = vec![
        (dd::subscription::TARGET, &target_nul),
        (dd::subscription::USER_NAME, &user_nul),
        (dd::subscription::PASSWORD, &password_enc),
        (dd::subscription::FORWARD, &forward_bytes),
        (dd::subscription::ENABLED, &enabled),
        (dd::subscription::QUEUE_NAME, &queue_nul),
        (dd::subscription::OPTIONS, &options_bytes),
        (dd::subscription::PAUSED, &paused),
    ];
    let mut sub_record = dd::create::object_record(&NewObject {
        id: sub_id,
        parent: 1,
        kind: "Subscriptn",
        name,
        info1: pub_id as i32,
        info2: 0x8000_0000,
        slots: &slots,
    });
    dd::create::set_comment_strlen(
        &mut sub_record,
        &dd::memo_path(&add_path),
        comment.as_bytes(),
    )
    .map_err(|_| Error::CorruptTable)?;

    // The queue table objects and files.
    let mut records = vec![sub_record];
    records.extend(queue_table_objects(table_id, name, &location));
    let folder = dictionary_folder(&add_path);
    let dict_file = add_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default();
    if let Some([data, memo, index]) = &location.files {
        write_queue_files(folder, dict_file, data, memo, index, table_id)?;
    }

    let next_autoinc = sub_id + 1 + QUEUE_FIELDS.len() as u32 + 3 + 1;
    dd::create::insert_objects(&add_path, &records, next_autoinc)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// The object id of the dictionary table the ADT file `path` is bound to
/// (`0x160` of its header), if it can be read.
fn bound_table_id(path: &Path) -> Option<u32> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(0x160)).ok()?;
    let mut id = [0u8; 4];
    file.read_exact(&mut id).ok()?;
    Some(u32::from_le_bytes(id))
}

/// Deletes a replication subscription (`AdsDDDeleteSubscription`): soft-deletes
/// its `Subscriptn` object and the whole replication queue table it owns (the
/// `__<name>` table, its fields, index and orders) in the engine's deletion
/// order, frees the subscription's comment memo, and removes the three queue
/// files — as the original engine does. A name that is no subscription is refused.
pub fn delete_subscription_api(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let subscription = dict
        .named(Kind::Subscription, name)
        .ok_or_else(|| not_valid("The subscription is not part of the database."))?;
    // The right to drop the subscription, then the one to drop its queue's
    // table, which goes with it (`fixtures/sql/ddapirights2`).
    super::rights::require(
        &conn,
        17,
        name,
        super::rights::DROP,
        "Permission to drop the subscription is not granted.",
    )?;
    let queue_table = format!("__{name}");
    if dict.named(Kind::Table, &queue_table).is_some() {
        super::rights::require(
            &conn,
            1,
            &queue_table,
            super::rights::DROP,
            "Table cannot be removed from the database.",
        )?;
    }
    let sub_record = subscription.record();
    // The queue table is found by the subscription's name as it is now: one
    // that was renamed has lost it, and its subscription alone is deleted (the
    // `__` table and its files stay, as the original leaves them).
    let table = dict.named(Kind::Table, &format!("__{name}"));
    let mut records: Vec<u32> = Vec::new();
    let mut queue_file = String::new();
    if let Some(table) = table {
        let queue = table.text(dd::table::RELATIVE_PATH).unwrap_or_default();
        // ".\q.add" -> "q.add". A queue with a folder has no files.
        queue_file = match queue.strip_prefix(".\\") {
            Some(file) if !file.contains('\\') => file.to_owned(),
            _ => String::new(),
        };
        // A queue file another subscription's table has since taken over
        // (made over the same name) is not this table's: its header says
        // which table it is bound to.
        if !queue_file.is_empty()
            && let Some(bound) = bound_table_id(&dictionary_folder(&add_path).join(&queue_file))
            && bound != table.id
        {
            return Err(ace_error::sql(
                5159,
                "The ADT table header is inconsistent with the data dictionary.",
            ));
        }
        // The queue objects in the engine's deletion order, the rights on the
        // table behind the table itself, then the subscription and its rights
        // (`fixtures/sql/ddapirights2`).
        for offset in QUEUE_DELETE_ORDER {
            // By object id: the records of a queue made after others were
            // deleted are not side by side.
            let Some(object) = dict.object(table.id + offset) else {
                continue;
            };
            records.push(object.record());
            if *offset == 0 {
                records.extend(permission_records(&dict, table.id));
            }
        }
    }
    records.push(sub_record);
    records.extend(permission_records(&dict, subscription.id));
    drop(dict);

    let folder = dictionary_folder(&add_path);
    if !queue_file.is_empty() {
        let (memo_file, index_file) = queue_companions(&queue_file);
        for file in [queue_file.as_str(), memo_file.as_str(), index_file.as_str()] {
            let _ = std::fs::remove_file(folder.join(file));
        }
    }
    delete_records(session, &conn, &add_path, &records)
}

/// Sets a property of a replication subscription (`AdsDDSetSubscriptionProperty`):
/// the target, user, enciphered password, queue, forward/enabled/paused flags,
/// the options and communication-type bits (in the options dword), the connection
/// string (enciphered), the user-defined property and the comment — as the
/// original engine stores them. An unknown subscription is refused; other
/// properties are [`Error::NotImplemented`].
pub fn set_subscription_property_api(
    session: &Session,
    connection: Handle,
    name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use dd::subscription::comm;
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    super::properties::require_known_property(
        property_id,
        super::properties::known_properties::SUBSCRIPTION,
        "a subscription object",
    )?;
    let dict = open_dict(&add_path)?;
    let subscription = find_named_api(&dict, Kind::Subscription, "subscription", name)?;
    super::rights::require(&conn, 17, name, super::rights::ALTER, "")?;
    let slot = subscription.record();
    // The current options dword, for the comm-bit and options read-modify-writes.
    let current_options = subscription.u32(dd::subscription::OPTIONS).unwrap_or(0);
    let key = super::connection_key(&conn, &dict);
    let scheme = dict.scheme();
    drop(dict);

    // A communication-type bit, set or cleared in the options dword.
    let comm_bit = |bit: u32| {
        let options = if value_u16(value) != 0 {
            current_options | bit
        } else {
            current_options & !bit
        };
        options.to_le_bytes()
    };
    match property_id {
        ADS_DD_COMMENT => {
            return set_replication_comment(session, &conn, &add_path, slot, comment_value(value));
        }
        1710 => {
            return set_object_slot(
                session,
                &conn,
                &add_path,
                slot,
                dd::subscription::OPTIONS,
                &comm_bit(comm::TCP_IP),
            );
        }
        1711 => {
            return set_object_slot(
                session,
                &conn,
                &add_path,
                slot,
                dd::subscription::OPTIONS,
                &comm_bit(comm::TCP_IP_V6),
            );
        }
        1712 => {
            return set_object_slot(
                session,
                &conn,
                &add_path,
                slot,
                dd::subscription::OPTIONS,
                &comm_bit(comm::UDP_IP),
            );
        }
        1713 => {
            return set_object_slot(
                session,
                &conn,
                &add_path,
                slot,
                dd::subscription::OPTIONS,
                &comm_bit(comm::IPX),
            );
        }
        1715 => {
            return set_object_slot(
                session,
                &conn,
                &add_path,
                slot,
                dd::subscription::OPTIONS,
                &comm_bit(comm::TLS),
            );
        }
        // ADS_DD_SUBSCR_OPTIONS: the plain options, keeping the comm bits.
        1707 => {
            let options = (value_u32(value) & !comm::MASK) | (current_options & comm::MASK);
            return set_object_slot(
                session,
                &conn,
                &add_path,
                slot,
                dd::subscription::OPTIONS,
                &options.to_le_bytes(),
            );
        }
        // ADS_DD_SUBSCR_PASSWORD / _CONNECTION_STR: enciphered with the key.
        1703 | 1716 => {
            let key = key.ok_or(Error::NotImplemented)?;
            let index = if property_id == 1703 {
                dd::subscription::PASSWORD
            } else {
                dd::subscription::CONNECTION_STR
            };
            let text = comment_value(value);
            let enc = tflip_storage::crypto::encipher_subscription_secret(
                scheme,
                &key,
                slot,
                index as u32,
                text,
            );
            return set_object_slot(session, &conn, &add_path, slot, index, &enc);
        }
        _ => {}
    }
    let index = match property_id {
        ADS_DD_USER_DEFINED_PROP => dd::subscription::USER_DEFINED,
        1701 => dd::subscription::TARGET,
        1702 => dd::subscription::USER_NAME,
        1704 => dd::subscription::FORWARD,
        1705 => dd::subscription::ENABLED,
        1706 | 1708 => dd::subscription::QUEUE_NAME,
        1709 => dd::subscription::PAUSED,
        1714 => dd::subscription::OPTIONS,
        _ => return Err(Error::NotImplemented),
    };
    set_object_slot(session, &conn, &add_path, slot, index, value)
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn creates_sets_and_deletes_publications_like_the_engine() {
        use tflip_storage::dd::{Dictionary, Kind};

        use crate::dictionary::{ObjectRef, Property};
        let session = Session::new();
        let fx = ops_fixtures();
        let base =
            tflip_storage::testing::temp_root().join(format!("tflip-ddpub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Compares a dictionary tflip built in `dir` with a golden the engine
        // made (the login secrets masked away).
        let compare = |dir: &Path, golden: &str| {
            for ext in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("db.{ext}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{golden}.{ext}"))).unwrap();
                let (ours, want) = match ext {
                    "add" => (mask_user_secrets(ours), mask_user_secrets(want)),
                    _ => (ours, want),
                };
                assert_eq!(ours, want, "{golden}.{ext}");
            }
        };
        let fresh = |name: &str| {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let add = dir.join("db.add");
            let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
            (dir, connect)
        };
        let property = |connect, object: ObjectRef, id| {
            connection::get(&session, connect)
                .unwrap()
                .with_database(|db| db.property(&object, id))
        };

        // Create: a publication with a comment, like the golden (create options
        // must be 0).
        let (dir, connect) = fresh("add");
        create_publication_api(&session, connect, "mypub", "a comment", 0).unwrap();
        compare(&dir, "pubadd");
        assert_eq!(
            property(connect, ObjectRef::Publication("mypub".to_owned()), 1500).unwrap(),
            Property::U32(0)
        );
        // A reused or empty name is refused.
        for name in ["mypub", "MYPUB", ""] {
            assert!(matches!(
                create_publication_api(&session, connect, name, "", 0),
                Err(Error::Sql(_))
            ));
        }
        connection::disconnect(&session, connect).ok();

        // Set: options, the user-defined property, then a first comment.
        let (dir, connect) = fresh("set");
        create_publication_api(&session, connect, "p", "", 0).unwrap();
        set_publication_property_api(&session, connect, "p", 1500, &0x20u32.to_le_bytes()).unwrap();
        set_publication_property_api(&session, connect, "p", 3, b"userprop\0").unwrap();
        set_publication_property_api(&session, connect, "p", 1, b"later\0").unwrap();
        compare(&dir, "pubset");
        assert_eq!(
            property(connect, ObjectRef::Publication("p".to_owned()), 1500).unwrap(),
            Property::U32(0x20)
        );
        assert_eq!(
            property(connect, ObjectRef::Publication("p".to_owned()), 3).unwrap(),
            Property::Text("userprop".to_owned())
        );
        // Setting a property of no publication fails.
        assert!(matches!(
            set_publication_property_api(&session, connect, "nope", 1500, &[0; 4]),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();

        // Delete: two publications, the first removed, as the golden has it.
        let (dir, connect) = fresh("del");
        create_publication_api(&session, connect, "p1", "", 0).unwrap();
        create_publication_api(&session, connect, "p2", "", 0).unwrap();
        delete_publication_api(&session, connect, "p1").unwrap();
        compare(&dir, "pubdel");
        {
            let dict = Dictionary::open(&dir.join("db.add")).unwrap();
            assert!(dict.named(Kind::Publication, "p1").is_none());
            assert!(dict.named(Kind::Publication, "p2").is_some());
        }
        assert!(matches!(
            delete_publication_api(&session, connect, "p1"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn creates_sets_and_deletes_articles_like_the_engine() {
        use tflip_storage::dd::{Dictionary, Kind};

        use crate::dictionary::{ObjectRef, Property};
        let session = Session::new();
        let fx = ops_fixtures();
        let base =
            tflip_storage::testing::temp_root().join(format!("tflip-ddart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let compare = |dir: &Path, golden: &str| {
            for ext in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("db.{ext}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{golden}.{ext}"))).unwrap();
                let (ours, want) = match ext {
                    "add" => (mask_user_secrets(ours), mask_user_secrets(want)),
                    _ => (ours, want),
                };
                assert_eq!(ours, want, "{golden}.{ext}");
            }
        };
        // A fresh dict with table t (id, s) added and a publication "pub", like
        // every article golden starts.
        let fresh = |name: &str| {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
            std::fs::copy(fx.join("t.adi"), dir.join("t.adi")).unwrap();
            let add = dir.join("db.add");
            let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
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
            create_publication_api(&session, connect, "pub", "", 0).unwrap();
            (dir, connect)
        };
        let property = |connect, id| {
            connection::get(&session, connect)
                .unwrap()
                .with_database(|db| db.property(&ObjectRef::Article("pub".into(), "t".into()), id))
        };

        // Create with a filter and explicit identifying columns.
        let (dir, connect) = fresh("add");
        create_article_api(&session, connect, "pub", "t", "id", "id > 5", 0, false).unwrap();
        compare(&dir, "artadd");
        assert_eq!(
            property(connect, 1600).unwrap(),
            Property::Text("id > 5".into())
        );
        assert_eq!(
            property(connect, 1601).unwrap(),
            Property::Text("id;".into())
        );
        assert_eq!(
            property(connect, 1602).unwrap(),
            Property::Bytes(vec![1, 0, 0, 0])
        );
        assert_eq!(
            property(connect, 1603).unwrap(),
            Property::Text("F1 > 5".into())
        );
        assert_eq!(
            property(connect, 1609).unwrap(),
            Property::Wide("id > 5".into())
        );
        // An unknown publication or table is refused.
        assert!(matches!(
            create_article_api(&session, connect, "nope", "t", "id", "", 0, false),
            Err(Error::Sql(_))
        ));
        assert!(matches!(
            create_article_api(&session, connect, "pub", "nope", "id", "", 0, false),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();

        // Identify by all columns, no explicit identifying columns.
        let (dir, connect) = fresh("all");
        create_article_api(&session, connect, "pub", "t", "", "s = 'x'", 2, false).unwrap();
        compare(&dir, "artall");
        assert_eq!(
            property(connect, 1601).unwrap(),
            Property::Text("id;s;".into())
        );
        assert_eq!(
            property(connect, 1603).unwrap(),
            Property::Text("F2 = 'x'".into())
        );
        connection::disconnect(&session, connect).ok();

        // An include list and the merge flags.
        let (dir, connect) = fresh("inc");
        create_article_api(&session, connect, "pub", "t", "id", "", 0, false).unwrap();
        set_article_property_api(&session, connect, "pub", "t", 1604, b"id;s\0").unwrap();
        set_article_property_api(&session, connect, "pub", "t", 1607, &1u16.to_le_bytes()).unwrap();
        set_article_property_api(&session, connect, "pub", "t", 1608, &1u16.to_le_bytes()).unwrap();
        compare(&dir, "artinc");
        assert_eq!(
            property(connect, 1604).unwrap(),
            Property::Text("id;s;".into())
        );
        assert_eq!(property(connect, 1607).unwrap(), Property::U16(1));
        assert_eq!(property(connect, 1608).unwrap(), Property::U16(1));
        // The include columns' numbers.
        assert_eq!(
            property(connect, 1606).unwrap(),
            Property::Bytes(vec![1, 0, 0, 0, 2, 0, 0, 0])
        );
        connection::disconnect(&session, connect).ok();

        // An exclude list.
        let (dir, connect) = fresh("exc");
        create_article_api(&session, connect, "pub", "t", "id", "", 0, false).unwrap();
        set_article_property_api(&session, connect, "pub", "t", 1605, b"s\0").unwrap();
        compare(&dir, "artexc");
        assert_eq!(
            property(connect, 1605).unwrap(),
            Property::Text("s;".into())
        );
        connection::disconnect(&session, connect).ok();

        // Delete.
        let (dir, connect) = fresh("del");
        create_article_api(&session, connect, "pub", "t", "id", "id > 5", 0, false).unwrap();
        delete_article_api(&session, connect, "pub", "t").unwrap();
        compare(&dir, "artdel");
        {
            let dict = Dictionary::open(&dir.join("db.add")).unwrap();
            assert!(
                dict.child(
                    dict.named(Kind::Publication, "pub").unwrap().id,
                    Kind::Article,
                    "00000002"
                )
                .is_none()
            );
        }
        connection::disconnect(&session, connect).ok();

        // The wide-filter create (AdsDDCreateArticle100) stores the same bytes.
        let (dir, connect) = fresh("w");
        create_article_api(&session, connect, "pub", "t", "id", "id > 5", 0, true).unwrap();
        compare(&dir, "art100");
        connection::disconnect(&session, connect).ok();

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn creates_sets_and_deletes_subscriptions_like_the_engine() {
        use tflip_storage::dd::{Dictionary, Kind};

        use crate::dictionary::{ObjectRef, Property};
        let session = Session::new();
        let fx = ops_fixtures();
        let base =
            tflip_storage::testing::temp_root().join(format!("tflip-ddsub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Compare the dictionary (secrets masked) and the queue files (exact).
        let compare = |dir: &Path, golden: &str, queue: bool| {
            for ext in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("db.{ext}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{golden}.{ext}"))).unwrap();
                let (ours, want) = match ext {
                    "add" => (mask_user_secrets(ours), mask_user_secrets(want)),
                    _ => (ours, want),
                };
                assert_eq!(ours, want, "{golden}.{ext}");
            }
            if queue {
                for (ours_ext, want_ext) in [("add", "add"), ("adm", "adm"), ("adi", "adi")] {
                    let ours = std::fs::read(dir.join(format!("q.{ours_ext}"))).unwrap();
                    let want = std::fs::read(fx.join(format!("{golden}_q.{want_ext}"))).unwrap();
                    assert_eq!(ours, want, "{golden}_q.{want_ext}");
                }
            }
        };
        let fresh = |name: &str, comment: &str, forward: u16| {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let add = dir.join("db.add");
            let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
            create_publication_api(&session, connect, "pub", "", 0).unwrap();
            create_subscription_api(
                &session,
                connect,
                "sub",
                "pub",
                "\\\\srv\\t.add",
                "u",
                "pw",
                "q.add",
                forward,
                comment,
                0,
            )
            .unwrap();
            (dir, connect)
        };
        let property = |connect, id| {
            connection::get(&session, connect)
                .unwrap()
                .with_database(|db| db.property(&ObjectRef::Subscription("sub".into()), id))
        };

        // Create a subscription (and its queue table) like the golden.
        let (dir, connect) = fresh("subadd", "cmt", 1);
        compare(&dir, "subadd", true);
        assert_eq!(
            property(connect, 1700).unwrap(),
            Property::Text("pub".into())
        ); // publication
        assert_eq!(
            property(connect, 1701).unwrap(),
            Property::Text("\\\\srv\\t.add".into())
        );
        assert_eq!(
            property(connect, 1703).unwrap(),
            Property::Text("pw".into())
        ); // deciphered
        assert_eq!(property(connect, 1704).unwrap(), Property::U16(1)); // forward
        assert_eq!(property(connect, 1705).unwrap(), Property::U16(1)); // enabled
        assert_eq!(
            property(connect, 1708).unwrap(),
            Property::Text("q.add".into())
        ); // queue
        // A reused or empty name, or an unknown publication, is refused.
        for (n, p) in [("sub", "pub"), ("", "pub"), ("s2", "nope")] {
            assert!(matches!(
                create_subscription_api(
                    &session, connect, n, p, "t", "u", "pw", "q2.add", 0, "", 0
                ),
                Err(Error::Sql(_))
            ));
        }
        connection::disconnect(&session, connect).ok();

        // Set properties: paused, the TCP/IP comm bit, a connection string and
        // the user-defined property, as the golden has them.
        let (dir, connect) = fresh("subset", "", 0);
        set_subscription_property_api(&session, connect, "sub", 1709, &1u16.to_le_bytes()).unwrap();
        set_subscription_property_api(&session, connect, "sub", 1710, &1u16.to_le_bytes()).unwrap();
        set_subscription_property_api(&session, connect, "sub", 1716, b"conn\0").unwrap();
        set_subscription_property_api(&session, connect, "sub", 3, b"ud\0").unwrap();
        compare(&dir, "subset", true);
        assert_eq!(property(connect, 1709).unwrap(), Property::U16(1)); // paused
        assert_eq!(property(connect, 1710).unwrap(), Property::U16(1)); // comm TCP/IP
        assert_eq!(property(connect, 1712).unwrap(), Property::U16(0)); // comm UDP off
        assert_eq!(property(connect, 1714).unwrap(), Property::U32(0x80)); // options internal (TCP/IP)
        assert_eq!(property(connect, 1707).unwrap(), Property::U32(0)); // options (comm masked)
        assert_eq!(
            property(connect, 1716).unwrap(),
            Property::Text("conn".into())
        ); // deciphered
        assert_eq!(property(connect, 3).unwrap(), Property::Text("ud".into())); // user-defined
        connection::disconnect(&session, connect).ok();

        // Delete: the subscription and its whole queue table go, and the queue
        // files are removed.
        let (dir, connect) = fresh("subdel", "cmt", 1);
        delete_subscription_api(&session, connect, "sub").unwrap();
        compare(&dir, "subdel", false);
        assert!(!dir.join("q.add").exists());
        assert!(!dir.join("q.adm").exists());
        assert!(!dir.join("q.adi").exists());
        {
            let dict = Dictionary::open(&dir.join("db.add")).unwrap();
            assert!(dict.named(Kind::Subscription, "sub").is_none());
            assert!(dict.named(Kind::Table, "__sub").is_none());
        }
        connection::disconnect(&session, connect).ok();

        let _ = std::fs::remove_dir_all(&base);
    }
    /// The texts of what is refused (`fixtures/sql/ddrepl`, whose replay
    /// compares the codes only).
    #[test]
    fn refuses_with_the_originals_texts() {
        let session = Session::new();
        let base = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddrefuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let add = base.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        create_publication_api(&session, connect, "pa", "", 0).unwrap();
        let said = |result: Result<(), Error>| match result {
            Err(Error::Sql(error)) => (error.native, error.message),
            other => panic!("{other:?}"),
        };
        let subscribe = |name: &str, publication: &str, queue: &str| {
            create_subscription_api(
                &session,
                connect,
                name,
                publication,
                "target",
                "usr",
                "pw",
                queue,
                0,
                "",
                0,
            )
        };

        let (code, message) = said(subscribe("s1", "nope", "q1.adt"));
        assert_eq!(code, 5132);
        assert!(
            message.ends_with("The publication does not exist."),
            "{message}"
        );
        subscribe("s1", "pa", "q1.adt").unwrap();
        let (_, message) = said(subscribe("S1", "pa", "q2.adt"));
        assert!(
            message.ends_with("Subscription already exists in the dictionary with the same name."),
            "{message}"
        );
        for queue in ["", "sub\\", "q3."] {
            let (code, message) = said(subscribe("s3", "pa", queue));
            assert_eq!(code, 5127);
            assert!(
                message.ends_with("The replication queue must be a file."),
                "{message}"
            );
        }
        let (code, message) = said(delete_publication_api(&session, connect, "pa"));
        assert_eq!(code, 5132);
        assert!(
            message.ends_with(
                "The publication is used by one or more subscriptions and cannot be deleted."
            ),
            "{message}"
        );
        let (_, message) = said(create_article_api(
            &session, connect, "nope", "t", "id", "", 0, false,
        ));
        assert!(
            message.ends_with("There is no such named publication in the data dictionary."),
            "{message}"
        );
        let (_, message) = said(create_article_api(
            &session, connect, "pa", "nope", "id", "", 0, false,
        ));
        assert!(
            message.ends_with("There is no such named object in the data dictionary."),
            "{message}"
        );
        let (_, message) = said(set_subscription_property_api(
            &session, connect, "nope", 1, b"x\0",
        ));
        assert!(
            message.ends_with("There is no such named subscription in the data dictionary."),
            "{message}"
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&base);
    }
}
