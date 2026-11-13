//! Data dictionaries: their objects (`AdsDDFindFirstObject`) and the
//! properties of the database and its objects (`AdsDDGet*Property`), read
//! from the dictionary a connection is to (see
//! [`tflip_engine::dictionary`]).
//!
//! The getters write a property as the original engine does:
//!
//! - Text with its NUL, `*len` counting it. Into a buffer too small for
//!   that, as much as fits, with `AE_INSUFFICIENT_BUFFER` and the length it
//!   needs. The `_W` properties are UTF-16, their lengths in bytes; too
//!   small a buffer is told two more than they need (and the original
//!   writes a stray byte after their first character, which tflip does not).
//! - A number only into a buffer of exactly its size: any other is
//!   `AE_INVALID_PROPERTY_LENGTH`, and the database, table, field, index
//!   and index file getters write back the size they need (the others
//!   leave `*len`). The referential integrity getter writes its rules, one
//!   byte each, into any buffer that holds them.
//! - A procedure's properties as stored, followed by one more NUL than
//!   they count (but the UTF-16 script).
//!
//! A property the object does not have is `AE_PROPERTY_NOT_SET` with
//! `*len` 0, one its kind of object does not have `AE_INVALID_PROPERTY_ID`,
//! and an object the dictionary does not have `AE_INVALID_OBJECT_NAME`. On a
//! connection to a folder, the getters fail with
//! `AE_INVALID_CONNECTION_HANDLE`.

use std::ffi::c_void;

use tflip_engine::dictionary::{ObjectRef, ObjectType, Property};

use crate::backend::api;
use crate::ffi::text;
use crate::last_error::{Call, fail, fail_detail, null_param};
use crate::{
    AE_INSUFFICIENT_BUFFER, AE_INVALID_OBJECT_NAME, AE_INVALID_OBJECT_TYPE, AE_INVALID_POINTER,
    AE_INVALID_PROPERTY_ID, AE_INVALID_PROPERTY_LENGTH, AE_NO_OBJECT_FOUND, AE_PERMISSION_DENIED,
    AE_PROPERTY_NOT_SET, AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16, Unsigned32, ansi, code,
    error_code,
};

/// `AE_INVALID_FIND_HANDLE`: the handle is no dictionary listing.
const AE_INVALID_FIND_HANDLE: Unsigned32 = 5129;

/// How a getter writes what does not fit.
#[derive(Debug, Clone, Copy, Default)]
struct Getter {
    /// Whether it writes back the size a number needs, given a buffer of
    /// another size.
    tells_size: bool,
    /// Whether a number fits any buffer that holds it.
    loose_numbers: bool,
    /// Whether it writes one more NUL than it counts (procedures).
    extra_nul: bool,
}

/// The database, table, field, index and index file getters.
const TELLING: Getter = Getter {
    tells_size: true,
    loose_numbers: false,
    extra_nul: false,
};
/// The trigger, user, group, view and link getters.
const SILENT: Getter = Getter {
    tells_size: false,
    loose_numbers: false,
    extra_nul: false,
};

/// What a getter writes back: the bytes for the start of the buffer, the
/// length for `*len` (`None` leaves it as it is) and the code it returns.
struct Answer {
    bytes: Vec<u8>,
    len: Option<Unsigned16>,
    rc: Unsigned32,
}

/// Reads the property `id` of `object` on `connection` and writes it to
/// `buf` of `*len` bytes, as `getter` does.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes. `position` is the number of the argument `buf` is, `len`
/// the one after it.
unsafe fn get(
    connection: AdsHandle,
    object: ObjectRef,
    id: Unsigned16,
    getter: Getter,
    buf: *mut c_void,
    len: *mut Unsigned16,
    position: u32,
) -> Unsigned32 {
    // Before the object and the property are looked at
    // (`fixtures/errors/param_n`).
    if buf.is_null() {
        return null_param(position);
    }
    if len.is_null() {
        return null_param(position + 1);
    }
    // SAFETY: `len` is null or readable, as the caller promises.
    let size = unsafe { len.as_ref() }.map(|&len| usize::from(len));
    let answer = property(connection, object, id, getter, size, !buf.is_null());
    // SAFETY: `buf` holds `size` bytes, as the caller promises; the answer
    // has no more bytes than that, none for a null `buf`, and a length only
    // for a `len` that is not null.
    unsafe { write_answer(answer, buf.cast(), len) }
}

/// The answer of [`get`] for a buffer of `size` bytes (`None` for a null
/// `len`) that is there or not (`has_buf`).
fn property(
    connection: AdsHandle,
    object: ObjectRef,
    id: Unsigned16,
    getter: Getter,
    size: Option<usize>,
    has_buf: bool,
) -> Answer {
    let only = |rc| Answer {
        bytes: Vec::new(),
        len: None,
        rc,
    };
    let Some(size) = size else {
        return only(fail(AE_INVALID_POINTER));
    };
    // Whether a table is encrypted is a number that does not say its size.
    let getter = match (&object, id) {
        (ObjectRef::Table(_), 214) => SILENT,
        _ => getter,
    };
    match api(connection).dd_property(connection, object.clone(), id) {
        Ok(_) | Err(tflip_engine::Error::PropertyNotSet)
            if anonymous(connection) && gated(&object, id) =>
        {
            only(fail(AE_PERMISSION_DENIED))
        }
        // The rights properties of a group: a success that leaves its mark
        // as the last error, whether it has rights granted or not.
        Ok(value) if matches!(&object, ObjectRef::Group(_)) && (1001..=1004).contains(&id) => {
            let answer = answer(value, getter, size, has_buf, &object, id);
            if answer.rc == AE_SUCCESS {
                fail(AE_NO_OBJECT_FOUND);
            }
            answer
        }
        // A rule's update and delete rules are one byte: a buffer of another
        // size gets them, and the length it should have as the last error.
        Ok(value)
            if matches!(&object, ObjectRef::Relation(_))
                && matches!(id, 505 | 506)
                && size != 1 =>
        {
            let answer = answer(value, getter, size, has_buf, &object, id);
            if answer.rc == AE_SUCCESS {
                fail_detail(AE_INVALID_PROPERTY_LENGTH, "Expected buffer length is 1.");
            }
            answer
        }
        // A subscription's password is no property to a connection with no
        // user, though it is a text to one with a user.
        Ok(_)
            if anonymous(connection)
                && matches!(&object, ObjectRef::Subscription(_))
                && id == 1703 =>
        {
            only(fail(AE_INVALID_PROPERTY_ID))
        }
        Ok(value) => answer(value, getter, size, has_buf, &object, id),
        Err(tflip_engine::Error::PropertyNotSet) => Answer {
            bytes: Vec::new(),
            len: Some(0),
            rc: fail(AE_PROPERTY_NOT_SET),
        },
        Err(tflip_engine::Error::InvalidObjectName) => only(match missing(connection, &object) {
            Some(detail) => fail_detail(AE_INVALID_OBJECT_NAME, &detail),
            None => fail(AE_INVALID_OBJECT_NAME),
        }),
        Err(tflip_engine::Error::InvalidPropertyId) => only(match unreadable(&object, id) {
            Some(detail) => fail_detail(AE_INVALID_PROPERTY_ID, detail),
            None => fail(AE_INVALID_PROPERTY_ID),
        }),
        Err(error) => only(error_code(error)),
    }
}

/// Whether the connection was opened with no user, which reads few of a
/// dictionary's properties (`fixtures/errors/param_n`).
pub(crate) fn anonymous(connection: AdsHandle) -> bool {
    api(connection)
        .connection_info(connection)
        .is_ok_and(|info| info.user.is_none())
}

/// Whether a connection with no user is refused the property `id` of
/// `object` (`AE_PERMISSION_DENIED`, before its size is looked at).
fn gated(object: &ObjectRef, id: Unsigned16) -> bool {
    match object {
        ObjectRef::Database => {
            matches!(
                id,
                103..=110 | 113 | 114 | 119 | 121 | 122 | 126 | 131 | 132
            )
        }
        ObjectRef::Table(_) => matches!(id, 203 | 205 | 207 | 208 | 209 | 211 | 215..=219),
        ObjectRef::Index(..) => matches!(id, 401..=403 | 406..=415),
        // The built-in groups have no rights to read.
        ObjectRef::Group(name) => !name.starts_with("DB:") && matches!(id, 1 | 3 | 1001..=1005),
        ObjectRef::Link(_) => matches!(id, 1300..=1303),
        ObjectRef::View(_) => matches!(id, 700..=704),
        ObjectRef::Procedure(_) => matches!(id, 802..=806),
        ObjectRef::Publication(_) => id == 1500,
        ObjectRef::Subscription(_) => matches!(id, 1700..=1702 | 1704..=1716),
        ObjectRef::Article(..) => matches!(id, 1600..=1609),
        ObjectRef::Relation(_) => matches!(id, 500..=508),
        _ => false,
    }
}

/// What the original adds to `AE_INVALID_OBJECT_NAME` for an object the
/// dictionary does not have.
fn missing(connection: AdsHandle, object: &ObjectRef) -> Option<String> {
    let exists = |object: ObjectRef| {
        !matches!(
            api(connection).dd_property(connection, object, 1),
            Err(tflip_engine::Error::InvalidObjectName)
        )
    };
    let no_table = "There is no such named table in the data dictionary.";
    let named = |kind: &str| format!("There is no such named {kind} in the data dictionary.");
    Some(match object {
        ObjectRef::Table(_) => no_table.to_owned(),
        ObjectRef::Field(table, _) if !exists(ObjectRef::Table(table.clone())) => {
            no_table.to_owned()
        }
        ObjectRef::Field(..) => "The specified field is not part of the table.".to_owned(),
        ObjectRef::Index(table, _) | ObjectRef::IndexFile(table, _)
            if !exists(ObjectRef::Table(table.clone())) =>
        {
            no_table.to_owned()
        }
        ObjectRef::User(_) => "There is no such user in the data dictionary.".to_owned(),
        ObjectRef::Group(_) => "There is no such user group in the data dictionary.".to_owned(),
        ObjectRef::Link(_) => named("link object"),
        ObjectRef::View(_) => named("view"),
        ObjectRef::Trigger(_) => named("Trigger"),
        ObjectRef::Procedure(name) => {
            format!("There is no such named procedure in the data dictionary: {name}")
        }
        ObjectRef::Relation(_) => named("RI object"),
        ObjectRef::Publication(_) => named("publication"),
        ObjectRef::Article(publication, _) => {
            match exists(ObjectRef::Publication(publication.clone())) {
                true => named("article"),
                false => named("publication"),
            }
        }
        ObjectRef::Subscription(_) => named("subscription"),
        _ => return None,
    })
}

/// What the original adds to `AE_INVALID_PROPERTY_ID`.
fn unreadable(object: &ObjectRef, id: Unsigned16) -> Option<&'static str> {
    match object {
        ObjectRef::Table(_) => Some("The specified property cannot be read for a table object."),
        ObjectRef::IndexFile(..) => {
            Some("The specified property is not valid for index file object.")
        }
        // The administrator's own properties, though not the others' ids.
        ObjectRef::User(_) if matches!(id, 1 | 3 | 1005 | 1102 | 1105) => {
            Some("The specified property cannot be read.")
        }
        ObjectRef::Link(_) if matches!(id, 1..=3) => {
            Some("The specified property cannot be read for a link object.")
        }
        ObjectRef::Database if id == 101 => Some("The property cannot be read."),
        ObjectRef::Relation(_) => {
            Some("The specified property cannot be read for a relation object.")
        }
        _ => None,
    }
}

/// What the original adds to `AE_INVALID_PROPERTY_LENGTH` for a number of
/// `width` bytes.
fn wrong_length(object: &ObjectRef, id: Unsigned16, width: usize) -> Option<String> {
    let expected = format!("Expected buffer length is {width}.");
    match (object, id) {
        (ObjectRef::Database, 5 | 108 | 109 | 111 | 112 | 113 | 125 | 132) => {
            Some("The property is a 16-bit value.".to_owned())
        }
        (ObjectRef::Database, 103 | 104 | 106 | 107 | 110 | 119 | 120 | 122) => {
            Some("The property is a 16-bit Boolean value.".to_owned())
        }
        (ObjectRef::Database, 126 | 131) => Some("The property is a 32-bit value.".to_owned()),
        // One of the original's texts lacks its hyphen.
        (ObjectRef::View(_), 5) => Some("The property is a 16 bit Boolean value.".to_owned()),
        (ObjectRef::View(_), 701) => Some("The property is a 16-bit integer.".to_owned()),
        (ObjectRef::Publication(_), 1500) => Some("The property is a 32-bit integer.".to_owned()),
        (ObjectRef::Subscription(_), 1704 | 1705 | 1709..=1713 | 1715) => {
            Some("The property is a 16-bit integer.".to_owned())
        }
        (ObjectRef::Subscription(_), 1707 | 1714) => {
            Some("The property is a 32-bit integer.".to_owned())
        }
        (ObjectRef::Article(..), 1607 | 1608) => Some(expected),
        (ObjectRef::Table(_), 215) => {
            Some("The memo block size property is a 2 byte value.".to_owned())
        }
        (ObjectRef::Table(_), 219) => {
            Some("The table caching property is a 2 byte value.".to_owned())
        }
        (ObjectRef::Table(_), 212) => Some("Table type is a 2 byte value.".to_owned()),
        (ObjectRef::Table(_), 214) => {
            Some("The table encryption flag is a 2 byte Boolean value.".to_owned())
        }
        (ObjectRef::Table(_), 5 | 203 | 204 | 207 | 208 | 216 | 220 | 222 | 223) => Some(expected),
        // One of the original's texts lacks its period.
        (ObjectRef::Field(..), 309 | 310) => Some(format!("Expected buffer length is {width}")),
        (ObjectRef::Field(..), 301 | 306..=308) => Some(expected),
        (ObjectRef::Index(..), 403..=406) => Some(expected),
        (ObjectRef::IndexFile(..), 901 | 903) => Some(expected),
        _ => None,
    }
}

/// How `getter` writes `value` to a buffer of `size` bytes that is there or
/// not (`has_buf`; see the module documentation).
fn answer(
    value: Property,
    getter: Getter,
    size: usize,
    has_buf: bool,
    object: &ObjectRef,
    id: Unsigned16,
) -> Answer {
    let number = |bytes: &[u8]| {
        let fits = match getter.loose_numbers {
            true => size >= bytes.len(),
            false => size == bytes.len(),
        };
        if !fits || !has_buf {
            return Answer {
                bytes: Vec::new(),
                len: getter.tells_size.then_some(bytes.len() as Unsigned16),
                rc: match wrong_length(object, id, bytes.len()) {
                    Some(detail) => fail_detail(AE_INVALID_PROPERTY_LENGTH, &detail),
                    None => fail(AE_INVALID_PROPERTY_LENGTH),
                },
            };
        }
        Answer {
            bytes: bytes.to_vec(),
            len: Some(bytes.len() as Unsigned16),
            rc: AE_SUCCESS,
        }
    };
    // Text and stored bytes: `bytes`, of which a buffer they fit gets
    // the first `counted`, and `extra` after them; one they do not fit as
    // much as it holds.
    let text = |bytes: &[u8], counted: usize, extra: &[u8]| {
        let extra = if getter.extra_nul { extra } else { &[] };
        let fits = has_buf && size >= bytes.len();
        let mut written = Vec::new();
        if has_buf {
            if fits {
                written.extend_from_slice(&bytes[..counted]);
                written.extend_from_slice(&extra[..extra.len().min(size - counted)]);
            } else {
                written.extend_from_slice(&bytes[..bytes.len().min(size)]);
            }
        }
        let told = if fits { counted } else { bytes.len() };
        Answer {
            bytes: written,
            len: Some(told.min(usize::from(u16::MAX)) as Unsigned16),
            rc: if fits {
                AE_SUCCESS
            } else {
                fail(AE_INSUFFICIENT_BUFFER)
            },
        }
    };
    match value {
        // A number that is not set: a buffer of the wrong size is refused
        // first.
        Property::Unset(width) => {
            let fits = match getter.loose_numbers {
                true => size >= usize::from(width),
                false => size == usize::from(width),
            };
            if !fits {
                return number(&vec![0; usize::from(width)]);
            }
            Answer {
                bytes: Vec::new(),
                len: Some(0),
                rc: fail(AE_PROPERTY_NOT_SET),
            }
        }
        Property::U8(n) => number(&[n]),
        Property::U16(n) => number(&n.to_le_bytes()),
        Property::U32(n) => number(&n.to_le_bytes()),
        Property::Text(t) => {
            let mut bytes = ansi::from_str_lossy(&t);
            bytes.push(0);
            // Up to the first NUL, which a text kept with its own has
            // before the one added.
            let counted = bytes
                .iter()
                .position(|&b| b == 0)
                .map_or(bytes.len(), |i| i + 1);
            text(&bytes, counted, &[0])
        }
        Property::Wide(t) => {
            let mut bytes: Vec<u8> = t.encode_utf16().flat_map(u16::to_le_bytes).collect();
            bytes.extend_from_slice(&[0, 0]);
            let mut answer = text(&bytes, bytes.len(), &[0]);
            // As the original engine does: too small a buffer is told two
            // bytes more than the text needs.
            if answer.rc == AE_INSUFFICIENT_BUFFER {
                answer.len = answer.len.map(|len| len.saturating_add(2));
            }
            answer
        }
        Property::Bytes(b) => text(&b, b.len(), &[0]),
    }
}

/// Writes `answer` back: its bytes to `buf`, its length to `len`; returns
/// its code.
///
/// # Safety
///
/// `buf` must hold the answer's bytes (null only if there are none), and
/// `len` must be valid for writes if the answer has a length.
unsafe fn write_answer(answer: Answer, buf: *mut u8, len: *mut Unsigned16) -> Unsigned32 {
    if !answer.bytes.is_empty() {
        // SAFETY: as the caller promises.
        unsafe { std::ptr::copy_nonoverlapping(answer.bytes.as_ptr(), buf, answer.bytes.len()) };
    }
    if let Some(told) = answer.len {
        // SAFETY: as the caller promises.
        unsafe { *len = told };
    }
    answer.rc
}

/// `AdsDDGetDatabaseProperty`: a property (`ADS_DD_*`) of the data
/// dictionary a connection is to. The administrator's password
/// (`ADS_DD_ADMIN_PASSWORD`) is not read. See the module documentation.
///
/// # Safety
///
/// `property` must be null or hold `*len` bytes; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDGetDatabaseProperty(
    connection: AdsHandle,
    id: Unsigned16,
    property: *mut c_void,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsDDGetDatabaseProperty");
    // SAFETY: `property` and `len` are as # Safety says.
    unsafe {
        get(
            connection,
            ObjectRef::Database,
            id,
            TELLING,
            property,
            len,
            3,
        )
    }
}

/// Declares a property getter of objects named by one name.
macro_rules! getter_of_one {
    ($(#[$doc:meta])* $name:ident, $label:literal, $variant:ident, $null:literal, $getter:expr, $ty:ty) => {
        $(#[$doc])*
        ///
        /// # Safety
        ///
        /// The name must be null or point to a NUL-terminated string;
        /// `property` must be null or hold `*len` bytes; `len` must be null
        /// or valid for reads and writes.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name(
            connection: AdsHandle,
            name: *mut Unsigned8,
            id: Unsigned16,
            property: *mut $ty,
            len: *mut Unsigned16,
        ) -> Unsigned32 {
            let _call = Call::begin($label);
            // SAFETY: the name is null or a NUL-terminated string, as
            // # Safety says.
            let name = unsafe { text(name) };
            // What a null name is called differs by object
            // (`fixtures/errors/param_n`); a user's succeeds, as for the
            // connection's own user, with nothing written to the buffer.
            if name.is_none() {
                return match $null {
                    "" => AE_SUCCESS,
                    detail => fail_detail(AE_INVALID_POINTER, detail),
                };
            }
            let object = ObjectRef::$variant(name.unwrap_or_default());
            // SAFETY: `property` and `len` are as # Safety says.
            unsafe { get(connection, object, id, $getter, property.cast(), len, 4) }
        }
    };
}

/// Declares a property getter of a table's objects, named by the table and
/// the object.
macro_rules! getter_of_two {
    ($(#[$doc:meta])* $name:ident, $label:literal, $variant:ident, $null_table:literal, $null_name:literal) => {
        $(#[$doc])*
        ///
        /// # Safety
        ///
        /// The names must be null or point to NUL-terminated strings;
        /// `property` must be null or hold `*len` bytes; `len` must be null
        /// or valid for reads and writes.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name(
            connection: AdsHandle,
            table: *mut Unsigned8,
            name: *mut Unsigned8,
            id: Unsigned16,
            property: *mut c_void,
            len: *mut Unsigned16,
        ) -> Unsigned32 {
            let _call = Call::begin($label);
            // SAFETY: the names are null or NUL-terminated strings, as
            // # Safety says.
            let (table, name) = unsafe { (text(table), text(name)) };
            let Some(table) = table else {
                return fail_detail(AE_INVALID_POINTER, $null_table);
            };
            let Some(name) = name else {
                return fail_detail(AE_INVALID_POINTER, $null_name);
            };
            let object = ObjectRef::$variant(table, name);
            // SAFETY: `property` and `len` are as # Safety says.
            unsafe { get(connection, object, id, TELLING, property, len, 5) }
        }
    };
}

getter_of_one!(
    /// `AdsDDGetTableProperty`: a property of a table of the dictionary.
    AdsDDGetTableProperty, "AdsDDGetTableProperty", Table, "Table name", TELLING, c_void
);
getter_of_one!(
    /// `AdsDDGetViewProperty`: a property of a view.
    AdsDDGetViewProperty, "AdsDDGetViewProperty", View, "View name", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetProcedureProperty`: a property of a stored procedure.
    AdsDDGetProcedureProperty,
    "AdsDDGetProcedureProperty",
    Procedure,
    "Procedure name",
    Getter { extra_nul: true, ..SILENT },
    c_void
);
getter_of_one!(
    /// `AdsDDGetUserProperty`: a property of a user. Passwords are not
    /// read; a user's groups only for a user in none but `DB:Public`, as
    /// the dictionary keeps them encrypted.
    AdsDDGetUserProperty, "AdsDDGetUserProperty", User, "", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetUserGroupProperty`: a property of a group.
    AdsDDGetUserGroupProperty, "AdsDDGetUserGroupProperty", Group, "Parameter 2 is not a valid string.", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetLinkProperty`: a property of a link to another dictionary.
    AdsDDGetLinkProperty, "AdsDDGetLinkProperty", Link, "Parameter 2", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetTriggerProperty`: a property of a trigger, named
    /// `table::trigger` (a database trigger by its name).
    AdsDDGetTriggerProperty, "AdsDDGetTriggerProperty", Trigger, "Trigger name", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetPublicationProperty`: a property of a replication publication.
    AdsDDGetPublicationProperty, "AdsDDGetPublicationProperty", Publication, "Parameter 2 is not a valid string.", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetSubscriptionProperty`: a property of a replication subscription.
    AdsDDGetSubscriptionProperty, "AdsDDGetSubscriptionProperty", Subscription, "Parameter 2 is not a valid string.", SILENT, c_void
);
getter_of_one!(
    /// `AdsDDGetRefIntegrityProperty`: a property of a referential
    /// integrity rule.
    AdsDDGetRefIntegrityProperty,
    "AdsDDGetRefIntegrityProperty",
    Relation,
    "Relation name",
    Getter { loose_numbers: true, ..SILENT },
    Unsigned8
);
getter_of_two!(
    /// `AdsDDGetArticleProperty`: a property of a replication article, named by
    /// its publication and the published table.
    AdsDDGetArticleProperty, "AdsDDGetArticleProperty", Article, "Parameter 2 is not a valid string.", "Parameter 3 is not a valid string."
);
getter_of_two!(
    /// `AdsDDGetFieldProperty`: a property of a field of a table.
    AdsDDGetFieldProperty, "AdsDDGetFieldProperty", Field, "Table name", "Field name"
);
getter_of_two!(
    /// `AdsDDGetIndexFileProperty`: a property of an index file of a table.
    AdsDDGetIndexFileProperty, "AdsDDGetIndexFileProperty", IndexFile, "Table name", "Parameter 3"
);
getter_of_two!(
    /// `AdsDDGetIndexProperty`: a property of an index of a table.
    AdsDDGetIndexProperty, "AdsDDGetIndexProperty", Index, "Parameter 2", "Parameter 3"
);

/// Writes an object's name, as the listing calls do: with its NUL, which
/// `*len` counts; into a buffer too small, as much as fits, with
/// `AE_INSUFFICIENT_BUFFER` and the length it needs.
///
/// # Safety
///
/// `buf` must hold `*len` bytes; `len` must be valid for reads and writes.
unsafe fn write_object_name(name: &str, buf: *mut Unsigned8, len: *mut Unsigned16) -> Unsigned32 {
    let property = Property::Text(name.to_owned());
    // SAFETY: `len` is readable, as the caller promises.
    let size = usize::from(unsafe { *len });
    // SAFETY: `buf` holds `size` bytes and `len` is writable, as the caller
    // promises.
    unsafe {
        write_answer(
            answer(property, SILENT, size, true, &ObjectRef::Database, 0),
            buf,
            len,
        )
    }
}

/// `AdsDDFindFirstObject`: starts a listing of the objects of a type
/// (`ADS_DD_*_OBJECT`) in a connection's data dictionary: the first name in
/// `name` and the listing in `*find`. Fields, index files, indexes and
/// (optionally) triggers are listed under their table, `parent`; fields in
/// their order in the table, the others in the order they were made.
/// Groups come with the built-in ones after them. Without objects of the
/// type, it fails with `AE_NO_OBJECT_FOUND`. Links are not followed: the
/// tables of a linked dictionary are not listed.
///
/// # Safety
///
/// `parent` must be null or point to a NUL-terminated string; `name` must
/// be null or hold `*len` bytes; `len` and `find` must be null or valid for
/// writes (`len` also for reads).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDFindFirstObject(
    connection: AdsHandle,
    kind: Unsigned16,
    parent: *mut Unsigned8,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
    find: *mut AdsHandle,
) -> Unsigned32 {
    let _call = Call::begin("AdsDDFindFirstObject");
    // SAFETY: `parent` is null or a NUL-terminated string, `find` null or
    // writable, as # Safety says.
    let (parent, find) = unsafe { (text(parent), find.as_mut()) };
    let Some(find) = find else {
        return null_param(6);
    };
    if name.is_null() {
        return null_param(4);
    }
    if len.is_null() {
        return null_param(5);
    }
    let Some(kind) = ObjectType::from_number(kind) else {
        return fail(AE_INVALID_OBJECT_TYPE);
    };
    match api(connection).find_first_object(connection, kind, parent) {
        Ok((first, handle)) => {
            *find = handle;
            // SAFETY: neither is null, so `name` holds `*len` bytes and
            // `len` is readable and writable, as # Safety says.
            unsafe { write_object_name(&first, name, len) }
        }
        Err(error) => crate::ddl::direct_error_code(error),
    }
}

/// `AdsDDFindNextObject`: the next name of a listing
/// [`AdsDDFindFirstObject`] started; after the last one,
/// `AE_NO_OBJECT_FOUND`.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDFindNextObject(
    _connection: AdsHandle,
    find: AdsHandle,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsDDFindNextObject");
    if name.is_null() {
        return null_param(3);
    }
    if len.is_null() {
        return null_param(4);
    }
    match api(find).find_next_table(find) {
        // SAFETY: neither is null, so `name` holds `*len` bytes and `len` is
        // readable and writable, as # Safety says.
        Ok(Some((next, _))) => unsafe { write_object_name(&next, name, len) },
        Ok(None) => fail(AE_NO_OBJECT_FOUND),
        Err(tflip_engine::Error::InvalidHandle) => fail(AE_INVALID_FIND_HANDLE),
        Err(error) => error_code(error),
    }
}

/// `AdsDDFindClose`: ends a listing [`AdsDDFindFirstObject`] started.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDDFindClose(_connection: AdsHandle, find: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsDDFindClose");
    // Taking a name off the listing is how a handle that is none is told
    // from one that is; it is closed next.
    if let Err(tflip_engine::Error::InvalidHandle) = api(find).find_next_table(find) {
        return fail(AE_INVALID_FIND_HANDLE);
    }
    code(api(find).find_close(find))
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use serde_json::{Value as Json, json};

    use super::*;
    use crate::fixtures::{self, remote_twins};
    use crate::{ADS_LOCAL_SERVER, ADS_REMOTE_SERVER, AdsConnect60, AdsDisconnect};

    /// The path a test connects to for the probe's dictionary `dd`
    /// (`dictionary/dict.add`), locally or on the server.
    fn dictionary(dd: &str) -> String {
        let (folder, file) = dd.split_once('/').unwrap();
        match fixtures::is_remote() {
            true => format!(r"{}\{file}", fixtures::source(folder)),
            false => fixtures::dir(folder).join(file).display().to_string(),
        }
    }

    /// Connects to `dd` as `user` with `password`.
    fn connect(dd: &str, user: Option<&str>, password: Option<&str>) -> (u32, AdsHandle) {
        let types = if fixtures::is_remote() {
            ADS_REMOTE_SERVER
        } else {
            ADS_LOCAL_SERVER
        };
        let path = CString::new(dictionary(dd)).unwrap();
        let user = user.map(|u| CString::new(u).unwrap());
        let password = password.map(|p| CString::new(p).unwrap());
        let ptr = |s: &Option<CString>| {
            s.as_ref()
                .map_or(std::ptr::null_mut(), |s| s.as_ptr().cast_mut().cast())
        };
        let mut handle = 0;
        let rc = unsafe {
            AdsConnect60(
                path.as_ptr().cast_mut().cast(),
                types,
                ptr(&user),
                ptr(&password),
                0,
                &mut handle,
            )
        };
        (rc, handle)
    }

    /// The probe's text argument `i` of `step`, or null.
    fn arg(step: &Json, i: usize) -> Option<CString> {
        step["args"][i].as_str().map(|s| CString::new(s).unwrap())
    }

    fn ptr(arg: &Option<CString>) -> *mut Unsigned8 {
        arg.as_ref()
            .map_or(std::ptr::null_mut(), |s| s.as_ptr().cast_mut().cast())
    }

    /// What a call wrote, as the probe records it: the data folder as
    /// `<data>` (with the path's separators backslashes), and the length
    /// less what that saves; `previous` says whether the call before (the
    /// same property into a large buffer) held the folder, for a path that
    /// did not fit, which starts as `<da`.
    fn written(step: &Json, rc: u32, len: u16, bytes: &[u8], previous: &mut bool) -> Json {
        let data = fixtures::dir("").display().to_string();
        let data = data.trim_end_matches('/');
        let text: String = bytes.iter().map(|&b| char::from(b)).collect();
        let mut len = i64::from(len);
        let mut value = bytes.to_vec();
        let had = *previous;
        *previous = false;
        if let Some(at) = text.find(data) {
            let rest = text[at + data.len()..].replace('/', "\\");
            let mut normal = text[..at].to_owned();
            normal.push_str("<data>");
            normal.push_str(&rest);
            value = normal.chars().map(|c| c as u8).collect();
            len -= data.len() as i64 - 6;
            *previous = true;
        } else if step["size"] == 3 && had && rc == AE_INSUFFICIENT_BUFFER {
            value = b"<da".to_vec();
            len -= data.len() as i64 - 6;
        }
        let mut result = json!({"rc": rc, "len": len, "value": hex(&value)});
        if let Some(size) = step.get("size") {
            result["size"] = size.clone();
        }
        result
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The bytes a call wrote into `buf`, up to the first untouched one.
    fn touched(buf: &[u8]) -> &[u8] {
        let end = buf.iter().rposition(|&b| b != 0x5A).map_or(0, |i| i + 1);
        &buf[..end]
    }

    /// Calls the property getter `step` names into a buffer of its size.
    fn property(step: &Json, connection: AdsHandle, previous: &mut bool) -> Json {
        let size = step["size"].as_u64().unwrap() as usize;
        let mut buf = vec![0x5Au8; size + 16];
        let mut len = size as u16;
        let out = buf.as_mut_ptr().cast::<c_void>();
        let args = step["args"].as_array().unwrap();
        let id = args.last().unwrap().as_u64().unwrap() as u16;
        let (a, b) = (arg(step, 0), arg(step, 1));
        let rc = unsafe {
            match step["fn"].as_str().unwrap() {
                "AdsDDGetDatabaseProperty" => {
                    AdsDDGetDatabaseProperty(connection, id, out, &mut len)
                }
                "AdsDDGetTableProperty" => {
                    AdsDDGetTableProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetViewProperty" => {
                    AdsDDGetViewProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetProcedureProperty" => {
                    AdsDDGetProcedureProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetUserProperty" => {
                    AdsDDGetUserProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetUserGroupProperty" => {
                    AdsDDGetUserGroupProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetLinkProperty" => {
                    AdsDDGetLinkProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetTriggerProperty" => {
                    AdsDDGetTriggerProperty(connection, ptr(&a), id, out, &mut len)
                }
                "AdsDDGetRefIntegrityProperty" => {
                    AdsDDGetRefIntegrityProperty(connection, ptr(&a), id, out.cast(), &mut len)
                }
                "AdsDDGetFieldProperty" => {
                    AdsDDGetFieldProperty(connection, ptr(&a), ptr(&b), id, out, &mut len)
                }
                "AdsDDGetIndexFileProperty" => {
                    AdsDDGetIndexFileProperty(connection, ptr(&a), ptr(&b), id, out, &mut len)
                }
                "AdsDDGetIndexProperty" => {
                    AdsDDGetIndexProperty(connection, ptr(&a), ptr(&b), id, out, &mut len)
                }
                other => panic!("{other}"),
            }
        };
        let limit = if size < 8000 { size + 4 } else { 8000 };
        let mut result = written(step, rc, len, touched(&buf[..limit]), previous);
        if size == 3 && rc == AE_INSUFFICIENT_BUFFER && is_wide(fun_id(step)) {
            result["value"] = json!("(not compared)");
        }
        result
    }

    /// The function and property a getter call names.
    fn fun_id(step: &Json) -> (&str, u64) {
        let id = step["args"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_u64()
            .unwrap();
        (step["fn"].as_str().unwrap(), id)
    }

    /// Whether the property is UTF-16 text, whose value the original
    /// engine writes a stray byte after when it does not fit.
    fn is_wide((fun, id): (&str, u64)) -> bool {
        match fun {
            "AdsDDGetFieldProperty" => (311..=313).contains(&id),
            "AdsDDGetTableProperty" => id == 221,
            "AdsDDGetIndexProperty" => (412..=415).contains(&id),
            "AdsDDGetViewProperty" => id == 704,
            "AdsDDGetProcedureProperty" => id == 806,
            "AdsDDGetTriggerProperty" => id == 1409,
            _ => false,
        }
    }

    /// The probe's calls tflip is known to answer otherwise (see
    /// KNOWN_ISSUES.md).
    fn known_difference(step: &Json) -> bool {
        let fun = step["fn"].as_str().unwrap();
        let args = &step["args"];
        let id = args
            .as_array()
            .and_then(|a| a.last())
            .and_then(Json::as_u64);
        match fun {
            // The original lists files in the process's folder for a mask.
            f if f.starts_with("AdsFindFirstTable") => !args[0].is_null(),
            // A user's groups are encrypted.
            "AdsDDGetUserProperty" => id == Some(1102),
            // Writes part of the rule's name.
            "AdsDDGetRefIntegrityProperty" => id == Some(500) && step["size"] == 3,
            _ => false,
        }
    }

    /// What tflip answers for the probe's calls on data dictionaries is
    /// what the original engine answers (`fixtures/dictionary/dd.json`,
    /// probed with `tools/ace/dd/`): connects, object listings, every
    /// property of every object into buffers of several sizes, tables and
    /// views opened by name, and table listings.
    /// No links are active, so every link number fails as in the original
    /// engine (probed); a free connection has no links at all. Nor are
    /// servers found.
    #[test]
    fn active_links_and_servers() {
        use crate::AdsGetActiveLinkInfo;
        let (rc, connection) = connect("dictionary/dict.add", Some("ADSSYS"), Some("adssyspw"));
        assert_eq!(rc, AE_SUCCESS);
        let (mut info, mut len) = ([0u8; 64], 64);
        let rc = unsafe { AdsGetActiveLinkInfo(connection, 1, info.as_mut_ptr(), &mut len) };
        assert_eq!(rc, crate::AE_INVALID_LINK_NUMBER);
        let null = std::ptr::null_mut();
        let rc = unsafe { AdsGetActiveLinkInfo(connection, 1, null, std::ptr::null_mut()) };
        assert_eq!(rc, crate::AE_INVALID_POINTER);
        AdsDisconnect(connection);
        let free = fixtures::connect("all_types");
        let rc = unsafe { AdsGetActiveLinkInfo(free, 1, info.as_mut_ptr(), &mut len) };
        assert_eq!(rc, crate::AE_NOT_DICTIONARY);
        AdsDisconnect(free);
        let mut table = 77;
        let find = crate::AdsFindServers;
        assert_eq!(unsafe { find(8, &mut table) }, crate::AE_INVALID_OPTION);
        assert_eq!(
            unsafe { find(0, std::ptr::null_mut()) },
            crate::AE_INVALID_POINTER
        );
        // Discovery returns the 13-column result table (zero rows when no
        // server answers, as here); the handle is a real, closable table.
        let mut table: AdsHandle = 0;
        assert_eq!(unsafe { find(3, &mut table) }, AE_SUCCESS);
        assert_ne!(table, 0);
        let mut fields: Unsigned16 = 0;
        assert_eq!(
            unsafe { crate::table::AdsGetNumFields(table, &mut fields) },
            AE_SUCCESS
        );
        assert_eq!(fields, 13);
        assert_eq!(crate::table::AdsCloseTable(table), AE_SUCCESS);
    }

    #[test]
    fn dictionaries_match_the_original_engine() {
        // Opening a table checks its indexes against the process's collations.
        let _settings = fixtures::settings_shared();
        let path = fixtures::dir("../dictionary/dd.json");
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut failures = Vec::new();
        let mut connection: AdsHandle = 0;
        let mut find: AdsHandle = 0;
        let mut table: AdsHandle = 0;
        let mut previous = false;
        let mut listing: (AdsHandle, String, Json) = (0, String::new(), Json::Null);
        for step in &steps {
            if known_difference(step) {
                continue;
            }
            let dd = step["dd"].as_str().unwrap();
            let fun = step["fn"].as_str().unwrap();
            let got = match fun {
                "AdsConnect60" => {
                    let (user, password) = match step["args"][0].as_str().unwrap() {
                        "none" => (None, None),
                        "ADSSYS, wrong" => (Some("ADSSYS"), Some("wrong")),
                        "adssys, lower case" => (Some("adssys"), Some("adssyspw")),
                        "unknown user" => (Some("nobody"), Some("x")),
                        "ADSSYS" => (Some("ADSSYS"), Some("adssyspw")),
                        other => panic!("{other}"),
                    };
                    let (rc, handle) = connect(dd, user, password);
                    if step["args"][0] == "ADSSYS" {
                        if connection != 0 {
                            AdsDisconnect(connection);
                        }
                        connection = handle;
                    } else if rc == AE_SUCCESS {
                        AdsDisconnect(handle);
                    }
                    json!({"rc": rc})
                }
                "AdsGetConnectionType" => {
                    let mut value = 0;
                    let rc = unsafe { crate::AdsGetConnectionType(connection, &mut value) };
                    // Through tflipd the connection is a remote one.
                    let value = if fixtures::is_remote() { 1 } else { value };
                    json!({"rc": rc, "value": value})
                }
                f if f.starts_with("AdsDDGet") => property(step, connection, &mut previous),
                "AdsDDFindFirstObject" | "AdsDDFindNextObject" => {
                    let mut buf = [0x5Au8; 260];
                    let short = step["args"].get(2).is_some();
                    let mut len: u16 = if short { 3 } else { 250 };
                    let rc = match fun {
                        "AdsDDFindFirstObject" => {
                            let kind = step["args"][0].as_u64().unwrap() as u16;
                            let parent = arg(step, 1);
                            let mut handle = 0;
                            let rc = unsafe {
                                AdsDDFindFirstObject(
                                    connection,
                                    kind,
                                    ptr(&parent),
                                    buf.as_mut_ptr(),
                                    &mut len,
                                    &mut handle,
                                )
                            };
                            if short && handle != 0 {
                                AdsDDFindClose(connection, handle);
                            } else {
                                find = handle;
                            }
                            rc
                        }
                        _ => unsafe {
                            AdsDDFindNextObject(connection, find, buf.as_mut_ptr(), &mut len)
                        },
                    };
                    let limit = if short { 8 } else { 254 };
                    json!({"rc": rc, "len": len, "value": hex(touched(&buf[..limit]))})
                }
                "AdsDDFindClose" => json!({"rc": AdsDDFindClose(connection, find)}),
                "AdsOpenTable" => {
                    if table != 0 {
                        crate::AdsCloseTable(table);
                        table = 0;
                    }
                    let name = arg(step, 0);
                    let rights = step["args"][1].as_u64().unwrap() as u16;
                    let null = std::ptr::null_mut();
                    let rc = unsafe {
                        crate::AdsOpenTable(
                            connection,
                            ptr(&name),
                            null,
                            0,
                            1,
                            0,
                            rights,
                            0,
                            &mut table,
                        )
                    };
                    json!({"rc": rc})
                }
                _ => table_call(step, table, &mut listing, connection, &mut previous),
            };
            let mut want = json!({"rc": step["rc"]});
            for key in ["len", "value", "size"] {
                if let Some(value) = step.get(key) {
                    want[key] = value.clone();
                }
            }
            if got["value"] == "(not compared)" {
                want["value"] = got["value"].clone();
            }
            // Static cursors are numbered in the process, so the length of
            // a name varies with the number of digits.
            let normal = |json: &Json| {
                let mut json = json.clone();
                if json["value"]
                    .as_str()
                    .is_some_and(|v| v.contains("74696d5f"))
                {
                    json["len"] = Json::Null;
                }
                tim_numbers(&json.to_string())
            };
            if normal(&got) != normal(&want) {
                failures.push(format!(
                    "{dd} {fun} {}\n want {want}\n  got {got}",
                    step["args"]
                ));
            }
        }
        if table != 0 {
            crate::AdsCloseTable(table);
        }
        AdsDisconnect(connection);
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// `text` with the hex of `tim_<n>` names as `tim_N`.
    fn tim_numbers(text: &str) -> String {
        // "tim_" is 74696d5f in hex; the number's hex digits (30-39, 61-66)
        // follow up to the NUL or ".tmp".
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find("74696d5f") {
            out.push_str(&rest[..at + 8]);
            rest = &rest[at + 8..];
            let mut digits = 0;
            while rest.len() >= digits + 2 {
                let pair = &rest[digits..digits + 2];
                if matches!(
                    pair,
                    "30" | "31"
                        | "32"
                        | "33"
                        | "34"
                        | "35"
                        | "36"
                        | "37"
                        | "38"
                        | "39"
                        | "61"
                        | "62"
                        | "63"
                        | "64"
                        | "65"
                        | "66"
                ) {
                    digits += 2;
                } else {
                    break;
                }
            }
            out.push('N');
            rest = &rest[digits..];
        }
        out.push_str(rest);
        out
    }

    /// The table and listing calls of the probe.
    fn table_call(
        step: &Json,
        table: AdsHandle,
        listing: &mut (AdsHandle, String, Json),
        connection: AdsHandle,
        previous: &mut bool,
    ) -> Json {
        let fun = step["fn"].as_str().unwrap();
        let args = step["args"].as_array().unwrap();
        let mut buf = [0x5Au8; 600];
        let mut len: u16 = 500;
        let index = |i: &Json| {
            let mut handle = 0;
            let n = i.as_u64().unwrap() as u16;
            unsafe { crate::AdsGetIndexHandleByOrder(table, n, &mut handle) };
            handle
        };
        let text = |rc: u32, len: u16, buf: &[u8], previous: &mut bool| {
            let limit = usize::from(len) + 2;
            written(
                step,
                rc,
                len,
                touched(&buf[..limit.min(buf.len())]),
                previous,
            )
        };
        let number = |rc: u32, value: u64| json!({"rc": rc, "value": value});
        unsafe {
            match fun {
                "AdsGetTableFilename" => {
                    let option = args[2].as_u64().unwrap() as u16;
                    let rc = crate::AdsGetTableFilename(table, option, buf.as_mut_ptr(), &mut len);
                    text(rc, len, &buf, previous)
                }
                "AdsGetTableAlias" => {
                    let rc = crate::AdsGetTableAlias(table, buf.as_mut_ptr(), &mut len);
                    text(rc, len, &buf, previous)
                }
                "AdsGetRecordCount" => {
                    let mut count = 0;
                    let rc = crate::AdsGetRecordCount(table, crate::ADS_IGNOREFILTERS, &mut count);
                    number(rc, u64::from(count))
                }
                "AdsGetNumFields" => {
                    let mut count = 0;
                    let rc = crate::AdsGetNumFields(table, &mut count);
                    number(rc, u64::from(count))
                }
                "AdsGetTableType" => {
                    let mut value = 0;
                    let rc = crate::AdsGetTableType(table, &mut value);
                    number(rc, u64::from(value))
                }
                "AdsGetNumIndexes" => {
                    let mut value = 0;
                    let rc = crate::AdsGetNumIndexes(table, &mut value);
                    number(rc, u64::from(value))
                }
                "AdsGetIndexName" => {
                    let rc = crate::AdsGetIndexName(index(&args[2]), buf.as_mut_ptr(), &mut len);
                    text(rc, len, &buf, previous)
                }
                "AdsGetIndexFilename" => {
                    let rc =
                        crate::AdsGetIndexFilename(index(&args[2]), 2, buf.as_mut_ptr(), &mut len);
                    text(rc, len, &buf, previous)
                }
                "AdsIsIndexPrimaryKey" => {
                    let mut value = 0x5A5A;
                    let rc = crate::AdsIsIndexPrimaryKey(index(&args[2]), &mut value);
                    number(rc, u64::from(value))
                }
                "AdsGetIndexFlags" => {
                    let mut value = 0;
                    let rc = crate::AdsGetIndexFlags(index(&args[2]), &mut value);
                    number(rc, u64::from(value))
                }
                // What the call before wrote to its dictionary name.
                "AdsFindFirstTable62 dd" => listing.2.clone(),
                "AdsFindFirstTable" | "AdsFindFirstTable62" => {
                    // The probe records the first name and each next one
                    // under the first call's name.
                    let mut dd = [0x5Au8; 300];
                    let mut dd_len: u16 = 250;
                    len = 250;
                    let null = std::ptr::null_mut();
                    let key = fun.to_owned();
                    let starting = listing.0 == 0 || listing.1 != key;
                    let rc = match (key.as_str(), starting) {
                        ("AdsFindFirstTable", true) => {
                            let mut handle = 0;
                            let rc = crate::AdsFindFirstTable(
                                connection,
                                null,
                                buf.as_mut_ptr(),
                                &mut len,
                                &mut handle,
                            );
                            listing.0 = handle;
                            listing.1 = key.clone();
                            rc
                        }
                        ("AdsFindFirstTable", false) => crate::AdsFindNextTable(
                            connection,
                            listing.0,
                            buf.as_mut_ptr(),
                            &mut len,
                        ),
                        (_, true) => {
                            let mut handle = 0;
                            let rc = crate::AdsFindFirstTable62(
                                connection,
                                null,
                                dd.as_mut_ptr(),
                                &mut dd_len,
                                buf.as_mut_ptr(),
                                &mut len,
                                &mut handle,
                            );
                            listing.0 = handle;
                            listing.1 = key.clone();
                            rc
                        }
                        (_, false) => crate::AdsFindNextTable62(
                            connection,
                            listing.0,
                            dd.as_mut_ptr(),
                            &mut dd_len,
                            buf.as_mut_ptr(),
                            &mut len,
                        ),
                    };
                    if rc != AE_SUCCESS {
                        listing.0 = 0;
                    }
                    listing.2 = json!({
                        "rc": rc,
                        "len": dd_len,
                        "value": hex(touched(&dd[..usize::from(dd_len) + 2])),
                    });
                    json!({"rc": rc, "len": len, "value": hex(touched(&buf[..usize::from(len) + 2]))})
                }
                other => panic!("{other}"),
            }
        }
    }

    /// Logging in checks the password as the original engine does
    /// (`fixtures/dictionary/login.json`, probed with `tools/ace/dd/login.c`):
    /// the user name is matched whatever its case, the password exactly, and a
    /// wrong, empty or missing password for a user that needs one fails with
    /// 7078; with no user name the password is not checked.
    #[test]
    fn login_matches_the_original_engine() {
        let path = fixtures::dir("../dictionary/login.json");
        let cases: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut failures = Vec::new();
        for case in &cases {
            let user = case["user"].as_str();
            let password = case["password"].as_str();
            let (rc, handle) = connect("dictionary/dict.add", user, password);
            if rc == AE_SUCCESS {
                AdsDisconnect(handle);
            }
            if rc != case["rc"].as_u64().unwrap() as u32 {
                failures.push(format!(
                    "user {:?} password {:?}: want {} got {rc}",
                    user, password, case["rc"]
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// A dictionary that verifies access rights enforces them on the connected
    /// user exactly as the original engine does (`fixtures/dictionary/rights.json`,
    /// probed with `tools/ace/dd/rights.c`): the administrator sees and changes
    /// everything; `bob`, with `SELECT`/`INSERT` on `parent` (through the group
    /// `readers`) and `UPDATE(label)` on `parent`, may open and read `parent`,
    /// update any of its columns and insert into it, but may not delete from it
    /// (5054 / 7200) and may not touch `child` at all (opening fails with 7079,
    /// a query with 7200). The replay runs the opens, the SQL and the table-API
    /// writes — the operations that carry an enforcement outcome — on a fresh
    /// copy, in the probe's order, and compares each return code.
    #[test]
    fn rights_enforcement_matches_the_original_engine() {
        use crate::{
            ADS_DEFAULT, AdsAppendRecord, AdsCancelUpdate, AdsCloseTable, AdsCreateSQLStatement,
            AdsDeleteRecord, AdsExecuteSQLDirect, AdsGotoTop, AdsOpenTable,
        };
        let cases: Vec<Json> = serde_json::from_slice(
            &std::fs::read(fixtures::dir("../dictionary/rights.json")).unwrap(),
        )
        .unwrap();
        let source = fixtures::scratch("rights", "dictionary_props");
        let path = match fixtures::is_remote() {
            true => format!(r"{source}\props.add"),
            false => std::path::PathBuf::from(&source)
                .join("props.add")
                .display()
                .to_string(),
        };
        let connect = |user: &str| {
            let types = if fixtures::is_remote() {
                ADS_REMOTE_SERVER
            } else {
                ADS_LOCAL_SERVER
            };
            let (p, u, pw) = (
                CString::new(path.clone()).unwrap(),
                CString::new(user).unwrap(),
                CString::new(if user == "ADSSYS" {
                    "adssyspw"
                } else {
                    "bobpw"
                })
                .unwrap(),
            );
            let mut h = 0;
            let rc = unsafe {
                AdsConnect60(
                    p.as_ptr().cast_mut().cast(),
                    types,
                    u.as_ptr().cast_mut().cast(),
                    pw.as_ptr().cast_mut().cast(),
                    0,
                    &mut h,
                )
            };
            assert_eq!(rc, AE_SUCCESS, "connect {user}");
            h
        };
        let open = |c: AdsHandle, name: &str, readonly: bool| {
            let n = CString::new(name).unwrap();
            let mut t = 0;
            let rc = unsafe {
                AdsOpenTable(
                    c,
                    n.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    ADS_DEFAULT,
                    1,
                    0,
                    1,
                    if readonly { 2 } else { 0 },
                    &mut t,
                )
            };
            (rc, t)
        };

        let mut failures = Vec::new();
        let mut connection: AdsHandle = 0;
        let mut current_user = String::new();
        let mut statement: AdsHandle = 0;
        let mut rw: AdsHandle = 0; // the table a table-API write sequence opened
        for case in &cases {
            let user = case["user"].as_str().unwrap();
            let op = case["op"].as_str().unwrap();
            if user != current_user {
                if connection != 0 {
                    AdsDisconnect(connection);
                }
                connection = connect(user);
                let mut s = 0;
                unsafe { AdsCreateSQLStatement(connection, &mut s) };
                statement = s;
                current_user = user.to_owned();
                rw = 0;
            }
            let want = case["rc"].as_u64().unwrap() as u32;
            // Only the operations that carry an enforcement outcome are replayed.
            let got = if let Some(rest) = op.strip_prefix("open ") {
                let (name, mode) = rest.rsplit_once(' ').unwrap();
                let (rc, t) = open(connection, name, mode == "readonly");
                if t != 0 {
                    AdsCloseTable(t);
                }
                Some(rc)
            } else if let Some(name) = op.strip_prefix("AdsOpenTable rw ") {
                let (rc, t) = open(connection, name, false);
                if rw != 0 {
                    AdsCloseTable(rw);
                }
                rw = t;
                Some(rc)
            } else if op.starts_with("AdsAppendRecord") {
                let rc = AdsAppendRecord(rw);
                if rc == AE_SUCCESS {
                    AdsCancelUpdate(rw);
                }
                Some(rc)
            } else if op.starts_with("AdsDeleteRecord") {
                AdsGotoTop(rw);
                let rc = AdsDeleteRecord(rw);
                if rw != 0 {
                    AdsCloseTable(rw);
                    rw = 0;
                }
                Some(rc)
            } else if op.starts_with("SELECT")
                || op.starts_with("UPDATE")
                || op.starts_with("INSERT")
                || op.starts_with("DELETE")
            {
                let sql = CString::new(op).unwrap();
                let mut cur = 0;
                let rc = unsafe {
                    AdsExecuteSQLDirect(statement, sql.as_ptr().cast_mut().cast(), &mut cur)
                };
                if cur != 0 {
                    AdsCloseTable(cur);
                }
                Some(rc)
            } else {
                None // AdsGetColumnPermissions / AdsGetField: not an enforcement outcome
            };
            if let Some(got) = got
                && got != want
            {
                failures.push(format!("{user} {op:?}: want {want} got {got}"));
            }
        }
        if connection != 0 {
            AdsDisconnect(connection);
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// tflip enforces a dictionary's referential integrity rules exactly as the
    /// original engine does (`fixtures/dictionary/ri.json`, probed with
    /// `tools/ace/dd/ri.c`): on the `dictionary_ri` fixture (a parent `p` and
    /// four children, one per rule — RESTRICT, CASCADE, SET NULL, SET DEFAULT),
    /// a child insert or update with a foreign key that has no parent is refused
    /// (7076 / 7200), a parent delete or key update applies its rule to the
    /// children (RESTRICT refuses, CASCADE deletes or re-points, SET NULL nulls,
    /// SET DEFAULT fails when the default has no parent), and a RESTRICT delete
    /// through the table API returns success and leaves the row. The replay runs
    /// each operation on a fresh copy, in the probe's order, and compares the
    /// return code, with the row count and first row of a check `SELECT`.
    #[test]
    fn ri_enforcement_matches_the_original_engine() {
        use crate::{
            ADS_DEFAULT, AdsAppendRecord, AdsAtEOF, AdsCloseTable, AdsCreateSQLStatement,
            AdsDeleteRecord, AdsExecuteSQLDirect, AdsGetField, AdsGetIndexHandle, AdsGotoTop,
            AdsIsNull, AdsOpenTable, AdsSeek, AdsSetDouble, AdsSkip, AdsWriteRecord,
        };
        let cases: Vec<Json> =
            serde_json::from_slice(&std::fs::read(fixtures::dir("../dictionary/ri.json")).unwrap())
                .unwrap();
        let source = fixtures::scratch("ri", "dictionary_ri");
        let path = match fixtures::is_remote() {
            true => format!(r"{source}\ri.add"),
            false => std::path::PathBuf::from(&source)
                .join("ri.add")
                .display()
                .to_string(),
        };
        let types = if fixtures::is_remote() {
            ADS_REMOTE_SERVER
        } else {
            ADS_LOCAL_SERVER
        };
        let (p, u, pw) = (
            CString::new(path).unwrap(),
            CString::new("ADSSYS").unwrap(),
            CString::new("").unwrap(),
        );
        let mut connection = 0;
        let rc = unsafe {
            AdsConnect60(
                p.as_ptr().cast_mut().cast(),
                types,
                u.as_ptr().cast_mut().cast(),
                pw.as_ptr().cast_mut().cast(),
                0,
                &mut connection,
            )
        };
        assert_eq!(rc, AE_SUCCESS, "connect");
        let mut statement = 0;
        unsafe { AdsCreateSQLStatement(connection, &mut statement) };

        let open_rw = |name: &str| -> AdsHandle {
            let n = CString::new(name).unwrap();
            let mut t = 0;
            unsafe {
                AdsOpenTable(
                    connection,
                    n.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    ADS_DEFAULT,
                    1,
                    0,
                    0,
                    0,
                    &mut t,
                )
            };
            t
        };
        // A check SELECT's row count and first row's `pid` (the probe's `check`),
        // so SET NULL, cascaded re-pointing and cascaded deletes are compared.
        let select_state = |sql: &str| -> (u32, u64, Json) {
            let s = CString::new(sql).unwrap();
            let mut cur = 0;
            let rc =
                unsafe { AdsExecuteSQLDirect(statement, s.as_ptr().cast_mut().cast(), &mut cur) };
            if rc != AE_SUCCESS || cur == 0 {
                return (rc, 0, Json::Null);
            }
            let mut rows = 0u64;
            let mut first = Json::Null;
            AdsGotoTop(cur);
            loop {
                let mut eof = 1u16;
                unsafe { AdsAtEOF(cur, &mut eof) };
                if eof != 0 {
                    break;
                }
                if rows == 0 {
                    let field = CString::new("pid").unwrap();
                    let mut isnull = 0u16;
                    unsafe { AdsIsNull(cur, field.as_ptr().cast_mut().cast(), &mut isnull) };
                    if isnull != 0 {
                        first = Json::String("NULL".into());
                    } else {
                        let mut buf = [0u8; 64];
                        let mut len = buf.len() as u32 - 1;
                        let frc = unsafe {
                            AdsGetField(
                                cur,
                                field.as_ptr().cast_mut().cast(),
                                buf.as_mut_ptr(),
                                &mut len,
                                0,
                            )
                        };
                        if frc == AE_SUCCESS {
                            let text = String::from_utf8_lossy(&buf[..len as usize]).into_owned();
                            first = Json::String(text);
                        }
                    }
                }
                rows += 1;
                AdsSkip(cur, 1);
            }
            AdsCloseTable(cur);
            (rc, rows, first)
        };

        let mut failures = Vec::new();
        for case in &cases {
            let op = case["op"].as_str().unwrap();
            let want = case["rc"].as_u64().unwrap() as u32;
            if op.starts_with("SELECT") {
                let (rc, rows, value) = select_state(op);
                let want_rows = case["rows"].as_u64().unwrap_or(0);
                let want_value = case["value"].clone();
                if rc != want || rows != want_rows || value != want_value {
                    failures.push(format!(
                        "{op:?}: want rc {want} rows {want_rows} value {want_value}, \
                         got rc {rc} rows {rows} value {value}"
                    ));
                }
                continue;
            }
            let got =
                if op.starts_with("UPDATE") || op.starts_with("INSERT") || op.starts_with("DELETE")
                {
                    let sql = CString::new(op).unwrap();
                    let mut cur = 0;
                    let rc = unsafe {
                        AdsExecuteSQLDirect(statement, sql.as_ptr().cast_mut().cast(), &mut cur)
                    };
                    if cur != 0 {
                        AdsCloseTable(cur);
                    }
                    rc
                } else if let Some(rest) = op.strip_prefix("API insert ") {
                    // "API insert <table> cid=<c> pid=<p>"
                    let mut parts = rest.split_whitespace();
                    let table = parts.next().unwrap();
                    let cid: f64 = parts
                        .next()
                        .unwrap()
                        .trim_start_matches("cid=")
                        .parse()
                        .unwrap();
                    let pid = parts.next().unwrap().trim_start_matches("pid=");
                    let t = open_rw(table);
                    AdsAppendRecord(t);
                    let (fc, fp) = (CString::new("cid").unwrap(), CString::new("pid").unwrap());
                    unsafe { AdsSetDouble(t, fc.as_ptr().cast_mut().cast(), cid) };
                    unsafe { AdsSetDouble(t, fp.as_ptr().cast_mut().cast(), pid.parse().unwrap()) };
                    let rc = AdsWriteRecord(t);
                    if rc != AE_SUCCESS {
                        crate::AdsCancelUpdate(t);
                    }
                    AdsCloseTable(t);
                    rc
                } else if let Some(rest) = op.strip_prefix("API delete p id=") {
                    let key = rest.trim();
                    let t = open_rw("p");
                    let tag = CString::new("p_pk").unwrap();
                    let mut ord = 0;
                    unsafe { AdsGetIndexHandle(t, tag.as_ptr().cast_mut().cast(), &mut ord) };
                    let keybuf = CString::new(key).unwrap();
                    let mut found = 0u16;
                    unsafe {
                        AdsSeek(
                            ord,
                            keybuf.as_ptr().cast_mut().cast(),
                            key.len() as u16,
                            2, // ADS_STRINGKEY
                            2, // ADS_HARDSEEK
                            &mut found,
                        )
                    };
                    let rc = AdsDeleteRecord(t);
                    AdsCloseTable(t);
                    rc
                } else {
                    continue;
                };
            if got != want {
                failures.push(format!("{op:?}: want {want} got {got}"));
            }
        }
        if connection != 0 {
            AdsDisconnect(connection);
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// tflip applies a dictionary's field defaults, field constraints, record
    /// validation and triggers on write exactly as the original engine does
    /// (`fixtures/dictionary/constraints.json`, probed with
    /// `tools/ace/dd/constraints.c` on the `dictionary_constraints` fixture):
    /// the replay runs the probe's operations in order on a fresh copy — table
    /// API appends, sets, writes and deletes, and SQL — and compares each
    /// step's return code, error text and the rows a `SELECT` returns.
    /// Recorded on the remote server, whose transactions take a failing
    /// trigger's writes back.
    #[test]
    fn constraints_match_the_original_engine() {
        // A failing trigger's writes are taken back only with real transactions.
        let _settings = fixtures::settings_shared();
        replay_rules("constraints.json", "constraints", "dictionary_constraints");
    }

    /// With transactions only counted (`[transactions] mode = count`), as the
    /// original's local server, which has none: a failing trigger keeps what
    /// it wrote (`fixtures/dictionary/constraints_local.json`). The mode is
    /// the process's, so the replay runs in a child process of its own.
    #[test]
    fn constraints_without_transactions_match_the_local_server() {
        replay_without_transactions("constraints_local");
    }

    /// tflip orders a dictionary's constraints, triggers and referential
    /// integrity, and reads its defaults, minimums and maximums, as the
    /// original engine does (`fixtures/dictionary/rules.json`, probed with
    /// `tools/ace/dd/constraints.c` running `rules.ops` on the
    /// `dictionary_rules` fixture): a record's constraints, then its BEFORE
    /// triggers, then its rules; a cascade, SET NULL or SET DEFAULT a child
    /// refuses leaves the parent as it was; defaults, minimums and maximums
    /// are expressions; and `AdsDDSetFieldProperty` refuses those a field
    /// cannot take. Recorded on the local server, so replayed with
    /// transactions only counted, in a child process.
    #[test]
    fn rules_match_the_local_server() {
        replay_without_transactions("rules");
    }

    /// Runs [`local_child`] on `golden` in a child process with
    /// `[transactions] mode = count` (the mode is the process's).
    fn replay_without_transactions(golden: &str) {
        let dir = std::env::temp_dir().join(format!("tflip-{golden}-count"));
        std::fs::create_dir_all(&dir).unwrap();
        let ini = dir.join("tflip.ini");
        std::fs::write(&ini, "[transactions]\nmode = count\n").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "dictionary::tests::local_child", "--ignored"])
            .args(["--test-threads", "1", "--quiet"])
            .env("TFLIP_CONFIG", &ini)
            .env("TFLIP_TEST_CONSTRAINTS_CHILD", golden)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "the local replay of {golden} failed: {status}"
        );
    }

    /// The child of [`replay_without_transactions`].
    #[test]
    #[ignore = "run by replay_without_transactions"]
    fn local_child() {
        match std::env::var("TFLIP_TEST_CONSTRAINTS_CHILD").as_deref() {
            Ok("constraints_local") => replay_rules(
                "constraints_local.json",
                "constraints_local",
                "dictionary_constraints",
            ),
            Ok("rules") => replay_rules("rules.json", "rules", "dictionary_rules"),
            _ => {}
        }
    }

    /// Replays the golden `golden` of `fixtures/dictionary/` on a copy of the
    /// dictionary fixture `fixture` in the scratch folder `scratch`.
    fn replay_rules(golden: &str, scratch: &str, fixture: &str) {
        use crate::{
            ADS_DEFAULT, AdsAppendRecord, AdsAtEOF, AdsCancelUpdate, AdsCloseTable,
            AdsCreateSQLStatement, AdsDeleteRecord, AdsExecuteSQLDirect, AdsGetField,
            AdsGetLastError, AdsGetNumFields, AdsGotoRecord, AdsGotoTop, AdsIsNull, AdsOpenTable,
            AdsSetEmpty, AdsSetField, AdsSetNull, AdsSkip, AdsWriteRecord,
        };
        let cases: Vec<Json> = serde_json::from_slice(
            &std::fs::read(fixtures::dir(&format!("../dictionary/{golden}"))).unwrap(),
        )
        .unwrap();
        let source = fixtures::scratch(scratch, fixture);
        let add = match fixture {
            "dictionary_rules" => "rules.add",
            _ => "constraints.add",
        };
        let path = match fixtures::is_remote() {
            true => format!(r"{source}\{add}"),
            false => std::path::PathBuf::from(&source)
                .join(add)
                .display()
                .to_string(),
        };
        let types = if fixtures::is_remote() {
            ADS_REMOTE_SERVER
        } else {
            ADS_LOCAL_SERVER
        };
        let (p, u, pw) = (
            CString::new(path).unwrap(),
            CString::new("ADSSYS").unwrap(),
            CString::new("").unwrap(),
        );
        let mut connection = 0;
        let rc = unsafe {
            AdsConnect60(
                p.as_ptr().cast_mut().cast(),
                types,
                u.as_ptr().cast_mut().cast(),
                pw.as_ptr().cast_mut().cast(),
                0,
                &mut connection,
            )
        };
        assert_eq!(rc, AE_SUCCESS, "connect");
        let mut statement = 0;
        unsafe { AdsCreateSQLStatement(connection, &mut statement) };

        let error_text = || -> Json {
            let mut code = 0;
            let mut buf = [0u8; 1024];
            let mut len = buf.len() as u16;
            unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
            let end = buf
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(len as usize)
                .min(len as usize);
            Json::String(crate::ansi::to_string(&buf[..end]))
        };
        // A field of the current record as trimmed text, NULL as null.
        let field_json = |t: AdsHandle, field: *mut u8| -> Json {
            let mut isnull = 0u16;
            let rc = unsafe { AdsIsNull(t, field, &mut isnull) };
            if rc != AE_SUCCESS {
                return json!({ "rc": rc });
            }
            if isnull != 0 {
                return Json::Null;
            }
            let mut buf = [0u8; 512];
            let mut len = buf.len() as u32 - 1;
            let rc = unsafe { AdsGetField(t, field, buf.as_mut_ptr(), &mut len, 0) };
            if rc != AE_SUCCESS {
                return json!({ "rc": rc });
            }
            Json::String(
                crate::ansi::to_string(&buf[..len as usize])
                    .trim_end()
                    .to_owned(),
            )
        };
        let open_rw = |name: &str| -> AdsHandle {
            let n = CString::new(name).unwrap();
            let mut t = 0;
            unsafe {
                AdsOpenTable(
                    connection,
                    n.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    ADS_DEFAULT,
                    1,
                    0,
                    0,
                    0,
                    &mut t,
                )
            };
            t
        };
        // Each `f=v` assignment set as the probe sets it, recording each step.
        let assign = |t: AdsHandle,
                      assigns: &[&str],
                      out: &mut serde_json::Map<String, Json>,
                      first: &mut u32| {
            for a in assigns {
                let Some((f, v)) = a.split_once('=') else {
                    continue;
                };
                let field = CString::new(f).unwrap();
                let fp = field.as_ptr().cast_mut().cast();
                let rc = match v {
                    "NULL" => unsafe { AdsSetNull(t, fp) },
                    "EMPTY" => unsafe { AdsSetEmpty(t, fp) },
                    _ => {
                        let value = CString::new(v).unwrap();
                        unsafe {
                            AdsSetField(t, fp, value.as_ptr().cast_mut().cast(), v.len() as u32)
                        }
                    }
                };
                out.insert(format!("set {f}"), json!(rc));
                if rc != AE_SUCCESS {
                    out.insert(format!("set {f} error"), error_text());
                    if *first == 0 {
                        *first = rc;
                    }
                }
            }
        };
        let finish = |t: AdsHandle, out: &mut serde_json::Map<String, Json>, first: u32| {
            let rc = AdsWriteRecord(t);
            out.insert("write".into(), json!(rc));
            out.insert("rc".into(), json!(if first != 0 { first } else { rc }));
            if rc != AE_SUCCESS {
                out.insert("error".into(), error_text());
                AdsCancelUpdate(t);
            }
        };

        let mut failures = Vec::new();
        for case in &cases {
            let op = case["op"].as_str().unwrap();
            let mut out = serde_json::Map::new();
            out.insert("op".into(), json!(op));
            let words: Vec<&str> = op.split(' ').collect();
            match words[0] {
                "sql" => {
                    let sql = CString::new(&op[4..]).unwrap();
                    let mut cur = 0;
                    let rc = unsafe {
                        AdsExecuteSQLDirect(statement, sql.as_ptr().cast_mut().cast(), &mut cur)
                    };
                    out.insert("rc".into(), json!(rc));
                    if rc != AE_SUCCESS {
                        out.insert("error".into(), error_text());
                    }
                    if rc == AE_SUCCESS && cur != 0 {
                        let mut n = 0u16;
                        unsafe { AdsGetNumFields(cur, &mut n) };
                        let mut rows = Vec::new();
                        AdsGotoTop(cur);
                        loop {
                            let mut eof = 1u16;
                            unsafe { AdsAtEOF(cur, &mut eof) };
                            if eof != 0 {
                                break;
                            }
                            let row: Vec<Json> = (1..=n)
                                .map(|i| field_json(cur, i as usize as *mut u8))
                                .collect();
                            rows.push(Json::Array(row));
                            AdsSkip(cur, 1);
                        }
                        out.insert("rows".into(), Json::Array(rows));
                    }
                    if cur != 0 {
                        AdsCloseTable(cur);
                    }
                }
                "append" => {
                    let t = open_rw(words[1]);
                    let mut first = AdsAppendRecord(t);
                    out.insert("append".into(), json!(first));
                    assign(t, &words[2..], &mut out, &mut first);
                    finish(t, &mut out, first);
                    AdsCloseTable(t);
                }
                "peek" => {
                    let t = open_rw(words[1]);
                    let rc = AdsAppendRecord(t);
                    out.insert("rc".into(), json!(rc));
                    let values: Vec<Json> = words[2]
                        .split(',')
                        .map(|f| {
                            let field = CString::new(f).unwrap();
                            field_json(t, field.as_ptr().cast_mut().cast())
                        })
                        .collect();
                    out.insert("values".into(), Json::Array(values));
                    AdsCancelUpdate(t);
                    AdsCloseTable(t);
                }
                "update" => {
                    let t = open_rw(words[1]);
                    let mut first = AdsGotoRecord(t, words[2].parse().unwrap());
                    out.insert("goto".into(), json!(first));
                    assign(t, &words[3..], &mut out, &mut first);
                    finish(t, &mut out, first);
                    AdsCloseTable(t);
                }
                "delete" => {
                    let t = open_rw(words[1]);
                    let mut rc = AdsGotoRecord(t, words[2].parse().unwrap());
                    if rc == AE_SUCCESS {
                        rc = AdsDeleteRecord(t);
                    }
                    out.insert("rc".into(), json!(rc));
                    if rc != AE_SUCCESS {
                        out.insert("error".into(), error_text());
                    }
                    AdsCloseTable(t);
                }
                "fprop" => {
                    // "fprop <table> <field> <property> <value...>"
                    let mut parts = op.splitn(5, ' ').skip(1);
                    let (table, field) = (parts.next().unwrap(), parts.next().unwrap());
                    let id: u16 = parts.next().unwrap().parse().unwrap();
                    let value = CString::new(parts.next().unwrap_or("")).unwrap();
                    let (table, field) =
                        (CString::new(table).unwrap(), CString::new(field).unwrap());
                    let length = value.as_bytes_with_nul().len() as u16;
                    let rc = unsafe {
                        crate::AdsDDSetFieldProperty(
                            connection,
                            table.as_ptr().cast_mut().cast(),
                            field.as_ptr().cast_mut().cast(),
                            id,
                            value.as_ptr().cast_mut().cast(),
                            length,
                            0,
                            std::ptr::null_mut(),
                        )
                    };
                    out.insert("rc".into(), json!(rc));
                    if rc != AE_SUCCESS {
                        out.insert("error".into(), error_text());
                    }
                }
                other => panic!("unknown op {other}"),
            }
            let got = Json::Object(out);
            if &got != case {
                failures.push(format!("want {case}\n got {got}"));
            }
        }
        if connection != 0 {
            AdsDisconnect(connection);
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    remote_twins!(
        dictionaries_match_the_original_engine,
        login_matches_the_original_engine,
        rights_enforcement_matches_the_original_engine,
        ri_enforcement_matches_the_original_engine,
        constraints_match_the_original_engine
    );
}
