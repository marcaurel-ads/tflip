//! The protocol between tflip's client library (`tflip-ace`) and its server
//! (`tflipd`).
//!
//! [`Api`] is the engine as a client sees it: operations on handles that
//! take and return owned values. The client library calls it for every
//! connection, whether the connection is in the process ([`Session`]
//! implements it, see [`local`]) or on a server ([`client::Client`]
//! implements it by sending each call as a [`Request`] and waiting for its
//! [`Reply`]). The server answers requests with [`dispatch`] on a session of
//! its own per client. So local and remote connections behave alike by
//! construction.
//!
//! Every operation is declared once, in the `api!` invocation below, which
//! generates the trait, the messages, the client and the dispatcher.
//!
//! The wire format is in [`wire`]; TLS, which every connection uses, is in
//! [`tls`].

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use tflip_engine::Error;
use tflip_engine::connection::ConnectionInfo;
use tflip_engine::dictionary::{ObjectRef, ObjectType, Property};
use tflip_engine::filter::{AofOptions, Evaluation};
use tflip_engine::getters::{FieldId, Null, Scalar, Want};
use tflip_engine::handle::{Activity, HandleKind, OpenTable};
use tflip_engine::index::IndexInfo;
use tflip_engine::seek::{SeekType, SeekValue};
use tflip_engine::setters::SetValue;
use tflip_engine::statement::{ParamRef, ParamValue, Setting as StatementSetting};
use tflip_engine::table::{Customize, LengthUnit, Position, TableInfo, TableSettings, TableType};
use tflip_storage::value::Value;

pub use rustls;
pub use tflip_engine::Session;
pub use tflip_engine::handle::Handle;

pub mod client;
pub mod deflate;
pub mod local;
#[cfg(feature = "server")]
pub mod server;
pub mod tls;
pub mod wire;

/// The version of the protocol, which client and server must agree on. Any
/// change to the operations below or to the types they carry changes it:
/// the wire format test (`tests/wire_format.rs`) fails on one until the
/// version is raised and the test's copies are renewed
/// (`TFLIP_BLESS=1 cargo test -p tflip-protocol --test wire_format`).
pub const PROTOCOL_VERSION: u16 = 88;

/// The port `tflipd` listens on unless told otherwise, the original
/// server's default.
pub const DEFAULT_PORT: u16 = 6262;

/// A key to seek or to bound a scope with, as the client gave it (see
/// [`SeekValue`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Key {
    /// The key's bytes, as the index stores them.
    Raw(Vec<u8>),
    /// Text, converted to the order's key type.
    Text(String),
    /// A number, for a numeric or date order.
    Double(f64),
}

impl Key {
    /// The key as the engine takes it.
    pub fn as_seek_value(&self) -> SeekValue<'_> {
        match self {
            Key::Raw(bytes) => SeekValue::Raw(bytes),
            Key::Text(text) => SeekValue::Text(text),
            Key::Double(number) => SeekValue::Double(*number),
        }
    }
}

impl From<SeekValue<'_>> for Key {
    fn from(value: SeekValue<'_>) -> Self {
        match value {
            SeekValue::Raw(bytes) => Key::Raw(bytes.to_vec()),
            SeekValue::Text(text) => Key::Text(text.to_owned()),
            SeekValue::Double(number) => Key::Double(number),
        }
    }
}

/// Declares the operations of [`Api`]. Each becomes:
/// - a method of [`Api`], returning `Result<T, Error>`;
/// - a variant of [`Request`] holding its arguments and one of [`Reply`]
///   holding its result, both named after the operation;
/// - a branch of [`dispatch`], which calls it for a request;
/// - a method of [`client::Client`], which sends its request.
macro_rules! api {
    ($(
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;
    )*) => {
        /// Every operation's signature, one per line in their order, which
        /// the wire format depends on.
        pub const SIGNATURES: &str =
            concat!($(stringify!(fn $name($($arg: $ty),*) -> $ret), "\n"),*);

        /// The engine as a client sees it. Handles are those of the
        /// session behind the implementation; handle 0 means the session
        /// itself where an operation allows it.
        pub trait Api {
            $(
                $(#[$doc])*
                #[allow(clippy::too_many_arguments)]
                fn $name(&self, $($arg: $ty),*) -> Result<$ret, Error>;
            )*
        }

        /// A call of an [`Api`] operation, as the client sends it.
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub enum Request {
            $( $name { $($arg: $ty),* }, )*
        }

        /// The result of a [`Request`], as the server answers it.
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub enum Reply {
            $( $name(Result<$ret, Error>), )*
        }

        /// Answers `request` by calling its operation on `api`.
        pub fn dispatch(api: &(impl Api + ?Sized), request: Request) -> Reply {
            match request {
                $( Request::$name { $($arg),* } => Reply::$name(api.$name($($arg),*)), )*
            }
        }

        /// An [`Api`] that fails every operation with one error.
        pub struct Refusing(pub Error);

        impl Api for Refusing {
            $(
                #[allow(unused_variables)]
                fn $name(&self, $($arg: $ty),*) -> Result<$ret, Error> {
                    Err(self.0.clone())
                }
            )*
        }

        impl Api for client::Client {
            $(
                fn $name(&self, $($arg: $ty),*) -> Result<$ret, Error> {
                    match self.call(Request::$name { $($arg),* })? {
                        Reply::$name(result) => result,
                        _ => Err(Error::ProtocolMismatch),
                    }
                }
            )*
        }
    };
}

api! {
    // Handles and connections.

    /// What `handle` refers to; `None` for an unknown handle.
    fn handle_kind(handle: Handle) -> Option<HandleKind>;
    /// Opens a connection described by a connect string
    /// (`Data Source=...;ServerType=...;`).
    fn connect(connect_string: String) -> Handle;
    /// Opens a connection to `path` on one of the servers `server_types`
    /// allows (`ADS_*_SERVER` bits; 0 for the session's setting), as `user`
    /// if given, with `password` (the administrator's, which recovers the
    /// dictionary key) if given, and the dictionary password `dd_password`
    /// that opens a dictionary encrypted with AES (`DDPassword`).
    fn connect_path(
        path: String,
        server_types: u16,
        user: Option<String>,
        password: Option<String>,
        dd_password: Option<String>,
    ) -> Handle;
    /// Closes a connection and everything opened on it; 0 closes every
    /// connection of the session.
    fn disconnect(connection: Handle) -> ();
    /// What does not change about a connection.
    fn connection_info(connection: Handle) -> ConnectionInfo;
    /// The collation tables opened on a connection without one take
    /// (`AdsSetCollation`); empty for none.
    fn collation(connection: Handle) -> String;
    /// Sets the collation tables opened on a connection without one take.
    fn set_collation(connection: Handle, collation: String) -> ();
    /// Sets whether a connection's writes enforce unique keys
    /// (`AdsDisableUniqueEnforcement`/`AdsEnableUniqueEnforcement`).
    fn set_unique_enforcement(connection: Handle, on: bool) -> ();
    /// Makes a connection refuse free tables to the users that are neither
    /// `ADSSYS` nor of `DB:Admin` (`CheckFreeTableAccess`, the connect option
    /// 0x2000).
    fn set_check_free_tables(connection: Handle, on: bool) -> ();
    /// Sets whether a connection's writes fill AutoInc fields
    /// (`AdsDisableAutoIncEnforcement`/`AdsEnableAutoIncEnforcement`).
    fn set_autoinc_enforcement(connection: Handle, on: bool) -> ();
    /// Keeps a value on a handle for the client (`AdsSetHandleLong`).
    fn set_handle_long(handle: Handle, value: u32) -> ();
    /// The value the client keeps on a handle.
    fn handle_long(handle: Handle) -> u32;
    /// The tables opened by name (not SQL cursors) and still open, in the
    /// order they were opened: those on a connection, or all for 0.
    fn tables(connection: Handle) -> Vec<Handle>;
    /// Starts a table listing: the first name matching `mask` in a
    /// connection's folder and the handle of the listing, or `None`.
    /// With the name, whether the listing lists a data dictionary's tables
    /// (a listing without a mask on a dictionary connection). With `links`
    /// (`AdsFindFirstTable62`), such a listing also lists the tables of the
    /// dictionary's links, each as `:link::table`.
    fn find_first_table(
        connection: Handle,
        mask: String,
        links: bool,
    ) -> Option<(String, Handle, bool)>;
    /// The links a dictionary connection has active, newest first: each
    /// one's name (empty for a dictionary's path), user and dictionary.
    fn active_links(connection: Handle) -> Vec<(String, String, String)>;
    /// The next name of a table listing, or `None` after the last.
    /// With the name, whether the listing lists a data dictionary's tables.
    fn find_next_table(find: Handle) -> Option<(String, bool)>;
    /// Ends a table listing; unknown handles are ignored.
    fn find_close(find: Handle) -> ();
    /// Starts a listing of the objects of a type in a connection's data
    /// dictionary, under `parent` (a table) if given: the first name and
    /// the handle of the listing, which goes on as a table listing does.
    fn find_first_object(
        connection: Handle,
        kind: ObjectType,
        parent: Option<String>,
    ) -> (String, Handle);
    /// A property (`ADS_DD_*`) of an object of a connection's data
    /// dictionary.
    fn dd_property(connection: Handle, object: ObjectRef, id: u16) -> Property;
    /// What the session has open.
    fn activity() -> Activity;
    /// The number of the session's connections; with `file`, of those with
    /// it open.
    fn users(file: Option<String>) -> u32;
    /// The tables the session has open: every table handle with
    /// `every_handle`, else each file once.
    fn open_tables(every_handle: bool) -> Vec<OpenTable>;
    /// The index files the session has open: of every table handle with
    /// `every_handle`, else each file once; with `table`, of every handle
    /// of that table.
    fn open_indexes(table: Option<String>, every_handle: bool) -> Vec<String>;

    // Settings.

    /// The date format of a connection; 0 for the session's.
    fn date_format(connection: Handle) -> String;
    /// Sets the date format of a connection; 0 sets the session's, and
    /// with it every connection's.
    fn set_date_format(connection: Handle, format: String) -> ();
    /// The session's epoch of years of two digits.
    fn epoch() -> u16;
    /// Sets the session's epoch of years of two digits.
    fn set_epoch(epoch: u16) -> ();
    /// Shows or hides the deleted records of the session's DBF tables.
    fn set_show_deleted(show: bool) -> ();
    /// The "exact" setting of a connection or table; 0 for the session's.
    fn exact(handle: Handle) -> bool;
    /// Sets the "exact" setting of a connection or table; 0 sets the
    /// session's, and with it every open connection's and table's.
    fn set_exact(handle: Handle, exact: bool) -> ();

    // Tables.

    /// Opens the table `name` on a connection (0: on its own).
    fn open_table(connection: Handle, name: String, settings: TableSettings) -> Handle;
    /// Closes a table and its index orders.
    fn close_table(table: Handle) -> ();
    /// What does not change about the table behind a table or index order
    /// handle.
    fn table_info(table: Handle) -> TableInfo;
    /// The number of records of the table behind `table`; for an index
    /// order with a scope and `scopes`, the keys in the scope; with
    /// `filters`, only the visible ones.
    fn record_count(table: Handle, scopes: bool, filters: bool) -> u32;
    /// The handles of a table's open index orders.
    fn indexes(table: Handle) -> Vec<Handle>;
    /// The handles of a table's full-text (FTS) indexes.
    fn full_text_indexes(table: Handle) -> Vec<Handle>;
    /// The local date the table's file was last written (a DBF table's
    /// header date), in the date format of the table's connection.
    fn last_update(table: Handle) -> String;
    /// A checksum of the current record; with `ignore_pages`, without the
    /// places of its memo values.
    fn record_crc(table: Handle, ignore_pages: bool) -> u32;
    /// A table's index orders as the original engine numbers them, its
    /// full-text indexes among them.
    fn numbered_indexes(table: Handle) -> Vec<Handle>;
    /// The last opened order of a table whose key expression is
    /// `expression`, ascending and without a condition.
    fn index_by_expression(table: Handle, expression: String) -> Option<Handle>;
    /// Opens an index file of a table: its orders' handles, and whether it
    /// was open already.
    fn open_index(table: Handle, name: String) -> (Vec<Handle>, bool);
    /// Closes the index file holding an order.
    fn close_index(order: Handle) -> ();
    /// Creates an ADT table from field definitions and opens it.
    fn create_table(
        connection: Handle,
        name: String,
        fields: String,
        memo_size: u16,
        settings: TableSettings,
    ) -> Handle;
    /// Creates a data dictionary at `path` (`AdsDDCreate`) and connects to it,
    /// returning the connection; `encrypt` is not supported yet.
    fn create_dictionary(path: String, encrypt: u16) -> Handle;
    /// Creates a data dictionary from a connect string
    /// (`AdsDDCreate101`): its `Data Source` the path, its `EncryptionType`
    /// (`AES128` or `AES256`) and `DDPassword` those of an AES dictionary.
    fn create_dictionary_connect(connect_string: String) -> Handle;
    /// Adds an existing table to the data dictionary `connection` is to
    /// (`AdsDDAddTable`): `name` the dictionary name, `path` the table file
    /// relative to the dictionary's folder, `table_type` its kind.
    fn add_table(
        connection: Handle,
        name: String,
        path: String,
        table_type: TableType,
        char_type: u16,
        index_files: String,
        comments: String,
    ) -> ();
    /// Adds a view to the data dictionary `connection` is to (`AdsDDAddView`):
    /// `name` its name, `sql` its statement, `comment` its comment (empty for
    /// none).
    fn add_view(connection: Handle, name: String, sql: String, comment: String) -> ();
    /// Adds a stored procedure to the dictionary (`AdsDDAddProcedure`): `name`
    /// its name, `container` the SQL script (when `invoke_option` has the script
    /// bit) or the external library name, `proc_name` the library function,
    /// `in_params`/`out_params` the parameter lists, `comment` its comment.
    fn add_procedure(
        connection: Handle,
        name: String,
        container: String,
        proc_name: String,
        invoke_option: u32,
        in_params: String,
        out_params: String,
        comment: String,
    ) -> ();
    /// Removes a stored procedure from the dictionary (`AdsDDRemoveProcedure`).
    fn remove_procedure(connection: Handle, name: String) -> ();
    /// Adds a trigger to the dictionary (`AdsDDCreateTrigger`): `name` its name,
    /// `table` the table it fires on, `trigger_type`/`event_types` when it fires,
    /// `container_type` and `container` its SQL script or external library,
    /// `function_name` the library function, `priority`, `comment` and `options`.
    fn add_trigger(
        connection: Handle,
        name: String,
        table: String,
        trigger_type: u32,
        event_types: u32,
        container_type: u32,
        container: String,
        function_name: String,
        priority: u32,
        comment: String,
        options: u32,
    ) -> ();
    /// Removes a trigger from the dictionary (`AdsDDRemoveTrigger`).
    fn remove_trigger(connection: Handle, name: String) -> ();
    /// Adds a referential-integrity rule to the dictionary
    /// (`AdsDDCreateRefIntegrity`/`62`): `name` its name, the parent and child
    /// tables and their index tags, the update and delete rules, and the
    /// no-primary and cascade error messages.
    fn add_refintegrity(
        connection: Handle,
        name: String,
        parent_table: String,
        parent_tag: String,
        child_table: String,
        child_tag: String,
        update_rule: u32,
        delete_rule: u32,
        no_primary_error: String,
        cascade_error: String,
    ) -> ();
    /// Removes a referential-integrity rule from the dictionary
    /// (`AdsDDRemoveRefIntegrity`).
    fn remove_refintegrity(connection: Handle, name: String) -> ();
    /// Adds a link to another data dictionary (`AdsDDCreateLink`): `alias` its
    /// name, `path` the linked dictionary, `user`/`password` the credentials and
    /// `options` the link options.
    fn add_link(
        connection: Handle,
        alias: String,
        path: String,
        user: String,
        password: String,
        options: u32,
    ) -> ();
    /// Removes a link from the dictionary (`AdsDDDropLink`).
    fn drop_link(connection: Handle, alias: String) -> ();
    /// Modifies a link in the dictionary (`AdsDDModifyLink`): rewrites the link
    /// `alias`'s linked-dictionary `path`, `user`/`password` credentials and
    /// `options`.
    fn modify_link(
        connection: Handle,
        alias: String,
        path: String,
        user: String,
        password: String,
        options: u32,
    ) -> ();
    /// Moves a table's files, or an index file, to a new directory
    /// (`AdsDDMoveObjectFile`): `object_type` the kind (1 table, 3 index file),
    /// `object_name` its name, `new_path` the destination directory,
    /// `index_files` a `;`-separated list of index files to move with a table,
    /// `parent` an index file's table, and `options` the move options.
    fn move_object_file(
        connection: Handle,
        object_type: u32,
        object_name: String,
        new_path: String,
        index_files: String,
        parent: String,
        options: u32,
    ) -> ();
    /// Creates a replication publication in the dictionary
    /// (`AdsDDCreatePublication`): `name` its name, `comment` its comment (empty
    /// for none), `options` its create options (0).
    fn create_publication(
        connection: Handle,
        name: String,
        comment: String,
        options: u32,
    ) -> ();
    /// Deletes a replication publication from the dictionary
    /// (`AdsDDDeletePublication`), with every article under it.
    fn delete_publication(connection: Handle, name: String) -> ();
    /// Sets a property of a replication publication
    /// (`AdsDDSetPublicationProperty`).
    fn set_publication_property(
        connection: Handle,
        name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Creates a replication article (`AdsDDCreateArticle`/`100`): `publication`
    /// the publication it joins, `table` the published table, `row_ident` the
    /// row-identifying columns (empty to use `options`), `filter` the row filter
    /// (empty for none), `options` the identify options.
    fn create_article(
        connection: Handle,
        publication: String,
        table: String,
        row_ident: String,
        filter: String,
        options: u32,
        wide: bool,
    ) -> ();
    /// Deletes a replication article (`AdsDDDeleteArticle`): the article for
    /// `table` under `publication`.
    fn delete_article(connection: Handle, publication: String, table: String) -> ();
    /// Sets a property of a replication article (`AdsDDSetArticleProperty`).
    fn set_article_property(
        connection: Handle,
        publication: String,
        table: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Creates a replication subscription (`AdsDDCreateSubscription`): `name` its
    /// name, `publication` the publication it is for, `target` the target
    /// database, `user`/`password` the target credentials, `queue` the
    /// replication queue, `forward` whether to forward replicated updates,
    /// `comment` its comment, `options` its options. Synthesizes the queue table.
    #[allow(clippy::too_many_arguments)]
    fn create_subscription(
        connection: Handle,
        name: String,
        publication: String,
        target: String,
        user: String,
        password: String,
        queue: String,
        forward: u16,
        comment: String,
        options: u32,
    ) -> ();
    /// Deletes a replication subscription (`AdsDDDeleteSubscription`), with its
    /// replication queue table.
    fn delete_subscription(connection: Handle, name: String) -> ();
    /// Sets a property of a replication subscription
    /// (`AdsDDSetSubscriptionProperty`).
    fn set_subscription_property(
        connection: Handle,
        name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Adds an index file to a table already in the dictionary
    /// (`AdsDDAddIndexFile`): `table` its table, `path` the index file relative
    /// to the dictionary's folder, `comment` its comment.
    fn add_index_file(connection: Handle, table: String, path: String, comment: String) -> ();
    /// Frees the table at `path` from its data dictionary (`AdsDDFreeTable`),
    /// clearing the binding in its own file; `password` is for an encrypted
    /// table.
    fn free_table(path: String, password: String) -> ();
    /// Removes a view from the dictionary (`AdsDDRemoveView`).
    fn remove_view(connection: Handle, name: String) -> ();
    /// Removes a table and all that belongs to it from the dictionary
    /// (`AdsDDRemoveTable`); `delete_files` removes its files too.
    fn remove_table(connection: Handle, name: String, delete_files: u16) -> ();
    /// Removes a table's index file from the dictionary (`AdsDDRemoveIndexFile`);
    /// `delete_file` removes the file too.
    fn remove_index_file(
        connection: Handle,
        table: String,
        index_file: String,
        delete_file: u16,
    ) -> ();
    /// Deletes one index order from a table in the dictionary
    /// (`AdsDDDeleteIndex`).
    fn dd_delete_index(connection: Handle, table: String, index: String) -> ();
    /// Renames a dictionary object of `object_type` (`AdsDDRenameObject`);
    /// `options` are its `ADS_KEEP_TABLE_FILE_NAME`.
    fn rename_object(
        connection: Handle,
        name: String,
        new_name: String,
        object_type: u16,
        options: u32,
    ) -> ();
    /// Creates a user group in the dictionary (`AdsDDCreateUserGroup`):
    /// `name` its name, `description` its description (empty for none).
    fn create_group(connection: Handle, name: String, description: String) -> ();
    /// Deletes a user group from the dictionary (`AdsDDDeleteUserGroup`).
    fn delete_group(connection: Handle, name: String) -> ();
    /// Creates a user in the dictionary (`AdsDDCreateUser`): `group` the group
    /// to add it to (empty for DB:Public only), `name` its name, `password` its
    /// password, `description` its description (empty for none).
    fn create_user(
        connection: Handle,
        group: String,
        name: String,
        password: String,
        description: String,
    ) -> ();
    /// Deletes a user from the dictionary (`AdsDDDeleteUser`).
    fn delete_user(connection: Handle, name: String) -> ();
    /// Adds a user to a group (`AdsDDAddUserToGroup`).
    fn add_user_to_group(connection: Handle, group: String, user: String) -> ();
    /// Removes a user from a group (`AdsDDRemoveUserFromGroup`).
    fn remove_user_from_group(connection: Handle, group: String, user: String) -> ();
    /// Grants `rights` on an object to a grantee (`AdsDDGrantPermission`).
    fn grant_permission(
        connection: Handle,
        object_type: u16,
        object_name: String,
        parent_name: String,
        grantee: String,
        rights: u32,
    ) -> ();
    /// Revokes `rights` on an object from a grantee (`AdsDDRevokePermission`).
    fn revoke_permission(
        connection: Handle,
        object_type: u16,
        object_name: String,
        parent_name: String,
        grantee: String,
        rights: u32,
    ) -> ();
    /// Sets an accessor's rights on an object to exactly what the access string
    /// `allowed_access` names (`AdsDDSetObjectAccessRights`); the object's type
    /// is inferred from its name.
    fn set_object_access_rights(
        connection: Handle,
        object_name: String,
        accessor_name: String,
        allowed_access: String,
    ) -> ();
    /// The permission mask a grantee holds on an object (`AdsDDGetPermissions`);
    /// `inherited` adds the grants of the groups a user belongs to.
    fn get_permissions(
        connection: Handle,
        grantee: String,
        object_type: u16,
        object_name: String,
        parent_name: String,
        inherited: bool,
    ) -> u32;
    /// Sets a property of a table in the dictionary (`AdsDDSetTableProperty`).
    /// Only the comment is handled yet.
    fn set_table_property(
        connection: Handle,
        table: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Sets a property of a view in the dictionary (`AdsDDSetViewProperty`).
    /// Only the comment is handled yet.
    fn set_view_property(
        connection: Handle,
        view: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Sets a property of a field in the dictionary (`AdsDDSetFieldProperty`).
    /// The comment and the string-valued field properties are handled.
    fn set_field_property(
        connection: Handle,
        table: String,
        field: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Sets a property of an index order in the dictionary
    /// (`AdsDDSetIndexProperty`). Only the comment is handled yet.
    fn set_index_property(
        connection: Handle,
        table: String,
        index: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Sets a property of the dictionary's database object
    /// (`AdsDDSetDatabaseProperty`). The comment and version numbers are
    /// handled.
    fn set_database_property(
        connection: Handle,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// `AdsDDSetUserProperty`: a user's comment, user-defined property, or the
    /// require-old-password flag.
    fn set_user_property(
        connection: Handle,
        user_name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// `AdsDDSetUserGroupProperty`: a group's comment or user-defined property.
    fn set_user_group_property(
        connection: Handle,
        group_name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// `AdsDDSetProcedureProperty`: a procedure's comment, parameters, DLL,
    /// invoke option or script.
    fn set_procedure_property(
        connection: Handle,
        procedure_name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> ();
    /// Changes the fields of a free table: those to add, delete and change,
    /// and the memo block size (0 keeps it).
    fn restructure_table(
        connection: Handle,
        name: String,
        settings: TableSettings,
        add: String,
        delete: String,
        change: String,
        memo_block_size: u32,
    ) -> ();
    /// Creates an index order on a table, in an index file (its structural
    /// one for `None`), and opens it.
    fn create_index(
        table: Handle,
        file: Option<String>,
        tag: String,
        expression: String,
        condition: String,
        options: u32,
        page_size: u32,
    ) -> Handle;
    /// Deletes an index order from its file.
    fn delete_index(order: Handle) -> ();
    /// Rebuilds the index files of a table, or of an order.
    fn reindex(handle: Handle) -> ();
    /// [`Api::reindex`] with ADI files of `page_size` bytes, 0 for their own.
    fn reindex_pages(handle: Handle, page_size: u32) -> ();
    /// Creates a full-text order on a table's field, in an index file (its
    /// structural one for `None`), and opens it: words of `min_word` to
    /// `max_word` letters; the delimiters, noise words, drop and conditional
    /// characters, each whether the defaults go with the list given; the
    /// `ADS_FTS_*` options.
    fn create_fts_index(
        table: Handle,
        file: Option<String>,
        tag: String,
        field: String,
        page_size: u32,
        min_word: u32,
        max_word: u32,
        lists: Vec<(bool, Option<String>)>,
        options: u32,
    ) -> ();
    /// Rebuilds the index files of a table that hold only full-text orders,
    /// ADI files with pages of `page_size` bytes (0 for their own).
    fn reindex_fts(table: Handle, page_size: u32) -> ();
    /// Packs a table, or with `zap` empties it; with memo blocks of
    /// `memo_block_size` bytes, 0 for the table's own.
    fn pack_table(table: Handle, zap: bool, memo_block_size: u32) -> ();
    /// Copies a table (an order's, a cursor) to a new file, its records
    /// (those the filter option takes) unless `structure_only`.
    fn copy_table(handle: Handle, filter: u16, name: String, structure_only: bool) -> ();
    /// Converts a table (an order's, a cursor) to a new file of another type.
    fn convert_table(
        handle: Handle,
        filter: u16,
        name: String,
        table_type: tflip_engine::table::TableType,
    ) -> ();
    /// Appends the records of a table to another, field by field name.
    fn copy_contents(from: Handle, to: Handle, filter: u16) -> ();
    /// Opens a table again, with its settings, on the record it is on.
    fn clone_table(table: Handle) -> Handle;
    /// Recalls every deleted record of a table opened exclusively.
    fn recall_all(table: Handle) -> ();
    /// Closes a table and deletes its files.
    fn delete_table(table: Handle) -> ();
    /// Makes a table transaction-free or not.
    fn set_transaction_free(table: Handle, free: bool) -> ();
    /// `AdsEnableEncryption`: set a table's RC4 encryption password (writes the
    /// header check and type, arms the cipher; does not encrypt existing data).
    fn enable_encryption(table: Handle, password: String) -> ();
    /// `AdsDisableEncryption`: drop the session cipher (no file change).
    fn disable_encryption(table: Handle) -> ();
    /// `AdsEncryptTable`: encipher every record and memo value and set the
    /// ENCRYPTED flag (open exclusively, password set).
    fn encrypt_table(table: Handle) -> ();
    /// `AdsDecryptTable`: decipher every record and memo value and clear the flag.
    fn decrypt_table(table: Handle) -> ();
    /// `AdsEncryptRecord`: encipher the current record (record-level).
    fn encrypt_record(table: Handle) -> ();
    /// `AdsDecryptRecord`: decipher the current record.
    fn decrypt_record(table: Handle) -> ();
    /// `AdsIsEncryptionEnabled`/`AdsVerifyPassword`: the session has the password.
    fn encryption_enabled(table: Handle) -> bool;
    /// `AdsIsRecordEncrypted`: the current record is enciphered (0x06/0x07).
    fn record_encrypted(table: Handle) -> bool;
    /// Closes a table's index files but its structural one.
    fn close_all_indexes(table: Handle) -> ();
    /// The index order of a table named `name`, regardless of case.
    fn index_by_name(table: Handle, name: String) -> Handle;
    /// An expression with field names replaced by field numbers (`F<n>`).
    fn expression_to_short(table: Handle, expression: String) -> String;
    /// An expression with field numbers (`F<n>`) replaced by field names.
    fn expression_to_long(table: Handle, expression: String) -> String;
    /// `AdsGetKeyFilter`: a filter identifying the current record by the unique
    /// index whose tag sorts first (`options` bit 1 = OData form). An empty
    /// `values_table` means none.
    fn key_filter(table: Handle, values_table: String, options: u32) -> String;

    // Moving. `table` is a table, moving in record order, or an index
    // order, moving in key order.

    /// Moves to the first record.
    fn goto_top(table: Handle) -> ();
    /// Moves to the last record.
    fn goto_bottom(table: Handle) -> ();
    /// Moves before the first record.
    fn goto_bof(table: Handle) -> ();
    /// Moves after the last record.
    fn goto_eof(table: Handle) -> ();
    /// Moves `records` records forward, or back if negative.
    fn skip(table: Handle, records: i64) -> ();
    /// Moves to record number `record`.
    fn goto_record(table: Handle, record: u32) -> ();
    /// Where the table is.
    fn position(table: Handle) -> Position;
    /// The record number of the position; with `visible`, counting the
    /// visible records only.
    fn record_number(table: Handle, visible: bool) -> u32;
    /// Whether the last move was a seek that found its key.
    fn found(table: Handle) -> bool;
    /// Whether the current record passes the filters.
    fn is_visible(table: Handle) -> bool;
    /// Whether the current record is deleted.
    fn is_deleted(table: Handle) -> bool;

    // Reading the current record. Fields are 1-based numbers.

    /// A field's value as a getter reading it as `want` gives it.
    fn field_as(table: Handle, field: FieldId, want: Want) -> Scalar;
    /// Whether a field is NULL, or with `empty` empty, in the current
    /// record.
    fn null(table: Handle, field: FieldId, empty: bool) -> Null;
    /// The raw bytes of the current record.
    fn read_record(table: Handle) -> Vec<u8>;
    /// The value a field holds.
    fn value(table: Handle, field: usize) -> Value;
    /// Whether a field holds NULL.
    fn is_null(table: Handle, field: usize) -> bool;
    /// Whether a field is empty (`AdsIsEmpty`).
    fn is_empty(table: Handle, field: usize) -> bool;
    /// The length in bytes of a memo field's value.
    fn memo_length(table: Handle, field: usize) -> u32;
    /// The value of a memo field, decompressed.
    fn read_memo(table: Handle, field: usize) -> Vec<u8>;
    /// The length of the data a field holds, counted in `unit`.
    fn data_length(table: Handle, field: usize, unit: LengthUnit) -> u32;

    // Writing records.

    /// Sets a field of the current record (`AdsSetString`, ...).
    fn set_field(table: Handle, field: FieldId, value: SetValue) -> ();
    /// Appends an empty record and goes to it.
    fn append_record(table: Handle) -> ();
    /// Writes the changes of the current record.
    fn write_record(table: Handle) -> ();
    /// Writes the current record's changes of a table not opened read-only.
    fn flush_table(table: Handle) -> ();
    /// Replaces the bytes of a table's current record but the first, or of
    /// the record an order's raw key is built from.
    fn set_record(handle: Handle, record: Vec<u8>) -> ();
    /// Writes the changes of every table of the session.
    fn write_all_records() -> ();
    /// Drops the changes of the current record.
    fn cancel_update(table: Handle) -> ();
    /// Deletes the current record.
    fn delete_record(table: Handle) -> ();
    /// Undoes a delete of the current record not written yet.
    fn recall_record(table: Handle) -> ();
    /// Locks a record, 0 for the current one.
    fn lock_record(table: Handle, record: u32) -> ();
    /// Releases a record lock, 0 for the current record.
    fn unlock_record(table: Handle, record: u32) -> ();
    /// Locks the table.
    fn lock_table(table: Handle) -> ();
    /// Releases the table lock.
    fn unlock_table(table: Handle) -> ();
    /// Whether the handle holds a lock on a record (0 for the current one)
    /// or the table.
    fn is_record_locked(table: Handle, record: u32) -> bool;
    /// Whether the handle holds the table lock.
    fn is_table_locked(table: Handle) -> bool;
    /// The records the handle holds locks on.
    fn locks(table: Handle) -> Vec<u32>;
    /// The AutoInc value of the last record the handle appended.
    fn last_autoinc(table: Handle) -> u32;

    // Index orders.

    /// What does not change about an index order.
    fn index_info(order: Handle) -> IndexInfo;
    /// The place of the table's record in an order, 1-based; with
    /// `scoped`, within its scope.
    fn key_number(order: Handle, scoped: bool) -> u32;
    /// The number of keys in an order; with `scoped`, in its scope.
    fn key_count(order: Handle, scoped: bool) -> u32;
    /// Where the current key is in an order, from 0 to 1.
    fn relative_position(order: Handle) -> f64;
    /// Moves to the key at `position` (0 to 1) of an order.
    fn set_relative_position(order: Handle, position: f64) -> ();
    /// Seeks `key` in an order; whether it was found.
    fn seek(order: Handle, key: Key, how: SeekType) -> bool;
    /// Seeks the last key of an order that starts with `key`; whether
    /// there is one.
    fn seek_last(order: Handle, key: Key) -> bool;
    /// Moves `records` distinct keys forward in an order, or back if
    /// negative.
    fn skip_unique(order: Handle, records: i64) -> ();
    /// Walks an order from its last key to its first, or not.
    fn set_index_direction(order: Handle, reversed: bool) -> ();
    /// The key of the current record in an order, its condition ignored.
    fn extract_key(order: Handle) -> Vec<u8>;
    /// The key a bookmark taken through an order holds.
    fn bookmark_key(order: Handle) -> Vec<u8>;
    /// Whether an order has a key, whatever its scope and the filters.
    fn lookup_key(order: Handle, key: Key) -> bool;
    /// The key a record, given whole, has in an order.
    fn key_from_record(order: Handle, record: Vec<u8>) -> Vec<u8>;
    /// Starts an order's raw key from a record of NULL fields.
    fn init_raw_key(order: Handle) -> ();
    /// Sets a field of the record an order's raw key is built from.
    fn set_key_field(order: Handle, field: FieldId, value: SetValue) -> ();
    /// The raw key built from the fields set, as long as `options` says.
    fn build_raw_key(order: Handle, options: u32) -> Vec<u8>;
    /// Adds the current record's key to a custom order.
    fn add_custom_key(order: Handle) -> ();
    /// Removes the current record's key from a custom order.
    fn delete_custom_key(order: Handle) -> ();
    /// Sets the top (or `bottom`) of an order's scope.
    fn set_scope(order: Handle, bottom: bool, key: Key) -> ();
    /// The top (or `bottom`) key of an order's scope.
    fn scope_key(order: Handle, bottom: bool) -> Vec<u8>;
    /// Clears the top (or `bottom`) of an order's scope.
    fn clear_scope(order: Handle, bottom: bool) -> ();
    /// Clears the scopes of every order of a table.
    fn clear_all_scopes(table: Handle) -> ();

    // Filters and expressions.

    /// Sets a table's Advantage Optimized Filter.
    fn set_aof(table: Handle, expression: String, options: AofOptions) -> ();
    /// A table's AOF.
    fn aof(table: Handle) -> String;
    /// How far a table's AOF is optimized (`ADS_OPTIMIZED_*`), and the part
    /// of it that is not.
    fn aof_level(table: Handle) -> (u16, String);
    /// How far `expression` would be optimized as a table's AOF.
    fn eval_aof(table: Handle, expression: String) -> u16;
    /// Removes a table's AOF.
    fn clear_aof(table: Handle) -> ();
    /// Sets a table's AOF aside, or puts it back.
    fn set_aof_active(table: Handle, active: bool) -> ();
    /// Drops the customization of a table's AOF.
    fn refresh_aof(table: Handle) -> ();
    /// Adds records to a table's AOF, removes them or toggles them.
    fn customize_aof(table: Handle, records: Vec<u32>, how: Customize) -> ();
    /// Whether record number `record` passes a table's AOF.
    fn is_in_aof(table: Handle, record: u32) -> bool;
    /// Sets a table's record filter.
    fn set_filter(table: Handle, expression: String) -> ();
    /// A table's record filter.
    fn filter(table: Handle) -> String;
    /// Removes a table's record filter.
    fn clear_filter(table: Handle) -> ();
    /// Checks that an expression compiles for a table.
    fn check_expression(table: Handle, expression: String) -> ();
    /// Evaluates an expression on a table's current record.
    fn eval(table: Handle, expression: String) -> Evaluation;
    /// Moves to the first visible record, from the top or the bottom, for
    /// which an expression holds; whether there is one.
    fn locate(table: Handle, expression: String, forward: bool) -> bool;
    /// Goes on with the table's last `locate` from its current record.
    fn continue_locate(table: Handle) -> bool;

    // Relations.

    /// Sets a relation from a table to an order of its child, keyed by an
    /// expression on the table, scoping the child's order to the key if
    /// `scoped`.
    fn set_relation(parent: Handle, order: Handle, expression: String, scoped: bool) -> ();
    /// Clears a table's relations.
    fn clear_relation(parent: Handle) -> ();

    // Transactions.

    /// Begins a transaction on a connection, or nests one; on every
    /// connection for 0.
    fn begin_transaction(connection: Handle) -> ();
    /// Commits a connection's transaction, or ends a level of it; for 0, of
    /// every connection that has one.
    fn commit_transaction(connection: Handle) -> ();
    /// Rolls a connection's transaction back, or to a savepoint; for 0, of
    /// every connection that has one.
    fn rollback_transaction(connection: Handle, savepoint: Option<String>) -> ();
    /// Sets a savepoint in a connection's transaction.
    fn create_savepoint(connection: Handle, name: String) -> ();
    /// A connection's transaction nesting level (0 outside one).
    fn transaction_count(connection: Handle) -> u32;
    /// Whether a connection is in a transaction; for 0, whether any is.
    fn in_transaction(connection: Handle) -> bool;
    /// Recovers the transactions a crash interrupted (see
    /// `tflip_engine::txlog`): how many.
    fn recover_transactions() -> u32;

    // SQL statements.

    /// Creates a statement on a connection.
    fn create_statement(connection: Handle) -> Handle;
    /// Closes a statement and its cursors.
    fn close_statement(statement: Handle) -> ();
    /// Prepares SQL on a statement.
    fn prepare(statement: Handle, sql: String) -> ();
    /// Checks SQL without running it.
    fn verify(statement: Handle, sql: String) -> ();
    /// Runs the prepared SQL; the cursor it returns, if any. Without
    /// `want_cursor`, a `SELECT` fails and keeps no cursor.
    fn execute(statement: Handle, want_cursor: bool) -> Option<Handle>;
    /// Prepares and runs SQL.
    fn execute_direct(statement: Handle, sql: String, want_cursor: bool) -> Option<Handle>;
    /// Sets a parameter of the prepared SQL.
    fn set_param(statement: Handle, param: ParamRef, value: ParamValue) -> ();
    /// Clears the values of the parameters.
    fn clear_params(statement: Handle) -> ();
    /// Changes a setting of the tables a statement opens.
    fn set_statement(statement: Handle, setting: StatementSetting) -> ();
    /// The SQL prepared on a statement.
    fn statement_text(statement: Handle) -> String;
    /// How many parameters a statement takes: the most of any text prepared.
    fn statement_params(statement: Handle) -> u32;
    /// The rows the last execution affected (`u32::MAX` after a `SELECT`).
    fn affected(statement: Handle) -> u32;
    /// The statement a cursor belongs to.
    fn cursor_statement(cursor: Handle) -> Handle;
    /// The cursor of a statement's last execution, if it is still open.
    fn open_cursor(statement: Handle) -> Option<Handle>;
    /// Whether a cursor is live (the table a `SELECT` reads).
    fn cursor_is_live(cursor: Handle) -> bool;

    // Reading a whole record at once.

    /// Every field of the current record read as `want`, so a client can
    /// cache a whole row from one call instead of one call per field. Each
    /// entry is what [`Api::field_as`] gives for that field in order;
    /// `None` for a memo-stored field (left to be read on demand, see
    /// [`tflip_engine::getters::row_cacheable`]).
    fn read_fields(table: Handle, want: Want) -> Vec<Option<Result<Scalar, Error>>>;

    // Keeping closed tables open for reuse.

    /// Sets how many tables (`AdsCacheOpenTables`), or SQL statements'
    /// tables with `cursors` (`AdsCacheOpenCursors`), a connection keeps
    /// open on the server after closing them.
    fn set_cache_size(connection: Handle, cursors: bool, size: u16) -> ();
    /// Closes the tables a connection keeps open after closing them
    /// (`AdsCloseCachedTables`).
    fn close_cached_tables(connection: Handle) -> ();
    /// Sets the data-encryption type of the tables created on a connection
    /// (the `EncryptionType` of its connect string: 0 for none, 5 for AES-128
    /// and 6 for AES-256), which a client of a server sends after connecting.
    fn set_encryption_type(connection: Handle, encryption_type: u8) -> ();
    /// Sets the table type of a connection's connect string (`TableType`),
    /// which a server's free-table backup opens the tables it finds as when
    /// the call maps no type to their extension.
    fn set_table_type(connection: Handle, table_type: TableType) -> ();
}
