//! [`Api`] on an engine session in the process: the one place that maps the
//! operations to engine calls. The client library uses it for local
//! connections, and the server for its clients' sessions.

use tflip_engine::connection::{self, ConnectionInfo};
use tflip_engine::dictionary::{ObjectRef, ObjectType, Property};
use tflip_engine::filter::{AofOptions, Evaluation};
use tflip_engine::getters::{self, FieldId, Null, Scalar, Want};
use tflip_engine::handle::{self, Activity, HandleKind, OpenTable};
use tflip_engine::index::{self, Index, IndexInfo};
use tflip_engine::seek::SeekType;
use tflip_engine::setters::SetValue;
use tflip_engine::statement::{self, ParamRef, ParamValue, Setting as StatementSetting};
use tflip_engine::table::{
    self, Customize, FieldRef, LengthUnit, Position, Table, TableInfo, TableSettings,
};
use tflip_engine::transaction;
use tflip_engine::{Error, Session, find, settings, trigger};
use tflip_storage::value::Value;

use crate::{Api, Handle, Key};

/// Calls `natural` on the table behind `table`, or `ordered` with the order
/// if `table` is one of its index orders.
fn moving<R>(
    session: &Session,
    table: Handle,
    natural: impl FnOnce(&Table) -> Result<R, Error>,
    ordered: impl FnOnce(&Table, &Index) -> Result<R, Error>,
) -> Result<R, Error> {
    let opened = table::get(session, table)?;
    match index::order_of(session, table) {
        // A cursor's temporary index is the order it shows its records in.
        Some(order) if !order.is_temporary() => ordered(&opened, &order),
        _ => natural(&opened),
    }
}

/// Calls `f` on an index order and the table it belongs to.
fn in_order<R>(
    session: &Session,
    order: Handle,
    f: impl FnOnce(&Table, &Index) -> Result<R, Error>,
) -> Result<R, Error> {
    let order = index::get(session, order)?;
    f(&*table::get(session, order.table)?, &order)
}

/// Answers the request in `frame` (one frame, as [`crate::wire`] writes it)
/// with `session`, and returns the reply as one frame; nothing if the frame
/// is not a request (the client then loses its connection). See
/// [`crate::client::Client::in_process`].
pub fn answer(session: &Session, frame: &[u8]) -> Vec<u8> {
    let mut reply = Vec::new();
    if let Ok(request) = crate::wire::read_message(&mut &frame[..]) {
        let _ = crate::wire::write_message(&mut reply, &crate::dispatch(session, request));
    }
    reply
}

impl Api for Session {
    fn handle_kind(&self, handle: Handle) -> Result<Option<HandleKind>, Error> {
        Ok(handle::kind(self, handle))
    }

    fn connect(&self, connect_string: String) -> Result<Handle, Error> {
        connection::connect(self, &connect_string)
    }

    fn connect_path(
        &self,
        path: String,
        server_types: u16,
        user: Option<String>,
        password: Option<String>,
        dd_password: Option<String>,
    ) -> Result<Handle, Error> {
        connection::connect_path_with(
            self,
            &path,
            server_types,
            user.as_deref(),
            password.as_deref(),
            dd_password.as_deref(),
        )
    }

    fn disconnect(&self, connection: Handle) -> Result<(), Error> {
        connection::disconnect(self, connection)
    }

    fn connection_info(&self, connection: Handle) -> Result<ConnectionInfo, Error> {
        Ok(connection::get(self, connection)?.info())
    }

    fn find_first_table(
        &self,
        connection: Handle,
        mask: String,
        links: bool,
    ) -> Result<Option<(String, Handle, bool)>, Error> {
        let first = find::find_first_table(self, connection, &mask, links)?;
        Ok(first.map(|first| {
            (
                first.name.to_string_lossy().into_owned(),
                first.find,
                first.dictionary,
            )
        }))
    }

    fn active_links(&self, connection: Handle) -> Result<Vec<(String, String, String)>, Error> {
        tflip_engine::link::active(self, connection)
    }

    fn find_next_table(&self, find: Handle) -> Result<Option<(String, bool)>, Error> {
        let next = find::find_next_table(self, find)?;
        Ok(next.map(|(name, dictionary)| (name.to_string_lossy().into_owned(), dictionary)))
    }

    fn collation(&self, connection: Handle) -> Result<String, Error> {
        Ok(connection::get(self, connection)?.collation())
    }

    fn set_collation(&self, connection: Handle, collation: String) -> Result<(), Error> {
        connection::get(self, connection)?.set_collation(&collation);
        Ok(())
    }

    fn set_cache_size(&self, connection: Handle, cursors: bool, size: u16) -> Result<(), Error> {
        connection::set_cache_size(self, connection, cursors, size)
    }

    fn close_cached_tables(&self, connection: Handle) -> Result<(), Error> {
        connection::close_cached_tables(self, connection)
    }

    fn set_unique_enforcement(&self, connection: Handle, on: bool) -> Result<(), Error> {
        connection::set_unique_enforcement(self, connection, on)
    }

    fn set_check_free_tables(&self, connection: Handle, on: bool) -> Result<(), Error> {
        connection::set_check_free_tables(self, connection, on)
    }

    fn set_encryption_type(&self, connection: Handle, encryption_type: u8) -> Result<(), Error> {
        connection::set_encryption_type(self, connection, encryption_type)
    }

    fn set_table_type(
        &self,
        connection: Handle,
        table_type: tflip_engine::table::TableType,
    ) -> Result<(), Error> {
        connection::set_table_type(self, connection, table_type)
    }

    fn set_autoinc_enforcement(&self, connection: Handle, on: bool) -> Result<(), Error> {
        connection::set_autoinc_enforcement(self, connection, on)
    }

    fn set_handle_long(&self, handle: Handle, value: u32) -> Result<(), Error> {
        Session::set_handle_long(self, handle, value)
    }

    fn handle_long(&self, handle: Handle) -> Result<u32, Error> {
        Session::handle_long(self, handle)
    }

    fn tables(&self, connection: Handle) -> Result<Vec<Handle>, Error> {
        if connection != 0 {
            connection::get(self, connection)?;
        }
        Ok(handle::tables(self, connection))
    }

    fn find_close(&self, find: Handle) -> Result<(), Error> {
        find::find_close(self, find);
        Ok(())
    }

    fn find_first_object(
        &self,
        connection: Handle,
        kind: ObjectType,
        parent: Option<String>,
    ) -> Result<(String, Handle), Error> {
        find::find_first_object(self, connection, kind, parent.as_deref())
    }

    fn dd_property(
        &self,
        connection: Handle,
        object: ObjectRef,
        id: u16,
    ) -> Result<Property, Error> {
        connection::get(self, connection)?.with_database(|db| db.property(&object, id))
    }

    fn field_as(&self, table: Handle, field: FieldId, want: Want) -> Result<Scalar, Error> {
        getters::field_as(&*table::get(self, table)?, &field, want)
    }

    fn null(&self, table: Handle, field: FieldId, empty: bool) -> Result<Null, Error> {
        getters::null(&*table::get(self, table)?, &field, empty)
    }

    fn activity(&self) -> Result<Activity, Error> {
        Ok(handle::activity(self))
    }

    fn users(&self, file: Option<String>) -> Result<u32, Error> {
        handle::users(self, file.as_deref())
    }

    fn open_tables(&self, every_handle: bool) -> Result<Vec<OpenTable>, Error> {
        Ok(handle::open_tables(self, every_handle))
    }

    fn open_indexes(
        &self,
        table: Option<String>,
        every_handle: bool,
    ) -> Result<Vec<String>, Error> {
        handle::open_indexes(self, table.as_deref(), every_handle)
    }

    fn epoch(&self) -> Result<u16, Error> {
        Ok(settings::epoch(self))
    }

    fn set_epoch(&self, epoch: u16) -> Result<(), Error> {
        settings::set_epoch(self, epoch);
        Ok(())
    }

    fn set_show_deleted(&self, show: bool) -> Result<(), Error> {
        settings::set_show_deleted(self, show);
        Ok(())
    }

    fn date_format(&self, connection: Handle) -> Result<String, Error> {
        connection::date_format(self, connection)
    }

    fn set_date_format(&self, connection: Handle, format: String) -> Result<(), Error> {
        connection::set_date_format(self, connection, &format)
    }

    fn exact(&self, handle: Handle) -> Result<bool, Error> {
        settings::exact_of(self, handle)
    }

    fn set_exact(&self, handle: Handle, exact: bool) -> Result<(), Error> {
        settings::set_exact_of(self, handle, exact)
    }

    fn open_table(
        &self,
        connection: Handle,
        name: String,
        settings: TableSettings,
    ) -> Result<Handle, Error> {
        table::open_for_client(self, connection, &name, settings)
    }

    fn close_table(&self, table: Handle) -> Result<(), Error> {
        table::close(self, table)
    }

    fn table_info(&self, table: Handle) -> Result<TableInfo, Error> {
        Ok(table::get(self, table)?.info())
    }

    /// Through an index order with a scope, the keys in the scope (with
    /// `scopes`); else an ADT table's records, deleted ones not counted. With
    /// `filters`, only the visible ones count. Handles that are not tables
    /// are [`Error::NotImplemented`] for now.
    fn record_count(&self, table: Handle, scopes: bool, filters: bool) -> Result<u32, Error> {
        let scoped =
            index::order_of(self, table).filter(|order| order.scope() != Default::default());
        if let Some(order) = scoped.filter(|_| scopes) {
            return table::get(self, order.table)?.key_count_in(&order, true);
        }
        // Through a binary order (`DELETED()`), as the original engine
        // counts: its keys, one per record slot and one more.
        if let Some(order) = index::order_of(self, table).filter(|order| order.tag.binary()) {
            return order.key_count(false);
        }
        let opened = table::get(self, table).map_err(|_| Error::NotImplemented)?;
        match filters {
            true => opened.visible_count(),
            false => opened.record_count(),
        }
    }

    fn indexes(&self, table: Handle) -> Result<Vec<Handle>, Error> {
        table::get(self, table)?;
        index::orders_of(self, table)
    }

    fn full_text_indexes(&self, table: Handle) -> Result<Vec<Handle>, Error> {
        index::full_text_of(self, table)
    }

    fn last_update(&self, table: Handle) -> Result<String, Error> {
        Ok(table::get(self, table)?.last_update_text())
    }

    fn set_field(&self, table: Handle, field: FieldId, value: SetValue) -> Result<(), Error> {
        table::get(self, table)?.set_field(field.as_ref(), &value)
    }

    fn append_record(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.append()
    }

    fn write_record(&self, table: Handle) -> Result<(), Error> {
        // A dictionary's constraints, referential integrity and triggers apply
        // around the write.
        trigger::write_record_api(self, &*table::get(self, table)?)
    }

    fn write_all_records(&self) -> Result<(), Error> {
        table::write_all(self)
    }

    fn cancel_update(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.cancel_update()
    }

    fn delete_record(&self, table: Handle) -> Result<(), Error> {
        // A dictionary's referential integrity and triggers apply around the
        // delete.
        trigger::delete_record_api(self, &*table::get(self, table)?)
    }

    fn flush_table(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.flush_buffers(self)
    }

    fn set_record(&self, handle: Handle, record: Vec<u8>) -> Result<(), Error> {
        match index::get(self, handle) {
            Ok(order) => order.set_raw_key_record(&record),
            Err(_) => table::get(self, handle)?.set_record(&record),
        }
    }

    fn recall_record(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.recall_record()
    }

    fn lock_record(&self, table: Handle, record: u32) -> Result<(), Error> {
        table::get(self, table)?.lock_record(record)
    }

    fn unlock_record(&self, table: Handle, record: u32) -> Result<(), Error> {
        table::get(self, table)?.unlock_record(record)
    }

    fn lock_table(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.lock_table()
    }

    fn unlock_table(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.unlock_table()
    }

    fn is_record_locked(&self, table: Handle, record: u32) -> Result<bool, Error> {
        table::get(self, table)?.is_record_locked(record)
    }

    fn is_table_locked(&self, table: Handle) -> Result<bool, Error> {
        Ok(table::get(self, table)?.is_table_locked())
    }

    fn locks(&self, table: Handle) -> Result<Vec<u32>, Error> {
        Ok(table::get(self, table)?.locks())
    }

    fn last_autoinc(&self, table: Handle) -> Result<u32, Error> {
        Ok(table::get(self, table)?.last_autoinc())
    }

    fn record_crc(&self, table: Handle, ignore_pages: bool) -> Result<u32, Error> {
        table::get(self, table)?.record_crc(ignore_pages)
    }

    fn numbered_indexes(&self, table: Handle) -> Result<Vec<Handle>, Error> {
        index::numbered(self, table)
    }

    fn index_by_expression(
        &self,
        table: Handle,
        expression: String,
    ) -> Result<Option<Handle>, Error> {
        index::by_expression(self, table, &expression)
    }

    fn open_index(&self, table: Handle, name: String) -> Result<(Vec<Handle>, bool), Error> {
        index::open_file(self, table, &name)
    }

    fn close_index(&self, order: Handle) -> Result<(), Error> {
        index::close_file(self, order)
    }

    fn create_table(
        &self,
        connection: Handle,
        name: String,
        fields: String,
        memo_size: u16,
        settings: TableSettings,
    ) -> Result<Handle, Error> {
        tflip_engine::ddl::create_table_in_dictionary_api(
            self, connection, &name, &fields, memo_size, settings,
        )
    }

    fn create_dictionary(&self, path: String, encrypt: u16) -> Result<Handle, Error> {
        tflip_engine::dictionary_write::create_dictionary_api(self, &path, encrypt)
    }

    fn create_dictionary_connect(&self, connect_string: String) -> Result<Handle, Error> {
        tflip_engine::dictionary_write::create_dictionary_connect_api(self, &connect_string)
    }

    fn add_table(
        &self,
        connection: Handle,
        name: String,
        path: String,
        table_type: tflip_engine::table::TableType,
        char_type: u16,
        index_files: String,
        comments: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_table_api(
            self,
            connection,
            &name,
            &path,
            table_type,
            char_type,
            &index_files,
            &comments,
        )
    }

    fn add_view(
        &self,
        connection: Handle,
        name: String,
        sql: String,
        comment: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_view_api(self, connection, &name, &sql, &comment)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_procedure(
        &self,
        connection: Handle,
        name: String,
        container: String,
        proc_name: String,
        invoke_option: u32,
        in_params: String,
        out_params: String,
        comment: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_procedure_api(
            self,
            connection,
            &name,
            &container,
            &proc_name,
            invoke_option,
            &in_params,
            &out_params,
            &comment,
        )
    }

    fn remove_procedure(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_procedure_api(self, connection, &name)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_trigger(
        &self,
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
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_trigger_api(
            self,
            connection,
            &name,
            &table,
            trigger_type,
            event_types,
            container_type,
            &container,
            &function_name,
            priority,
            &comment,
            options,
        )
    }

    fn remove_trigger(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_trigger_api(self, connection, &name)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_refintegrity(
        &self,
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
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_refintegrity_api(
            self,
            connection,
            &name,
            &parent_table,
            &parent_tag,
            &child_table,
            &child_tag,
            update_rule as u8,
            delete_rule as u8,
            &no_primary_error,
            &cascade_error,
        )
    }

    fn remove_refintegrity(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_refintegrity_api(self, connection, &name)
    }

    fn add_link(
        &self,
        connection: Handle,
        alias: String,
        path: String,
        user: String,
        password: String,
        options: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_link_api(
            self, connection, &alias, &path, &user, &password, options,
        )
    }

    fn drop_link(&self, connection: Handle, alias: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::drop_link_api(self, connection, &alias)
    }

    fn modify_link(
        &self,
        connection: Handle,
        alias: String,
        path: String,
        user: String,
        password: String,
        options: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::modify_link_api(
            self, connection, &alias, &path, &user, &password, options,
        )
    }

    fn move_object_file(
        &self,
        connection: Handle,
        object_type: u32,
        object_name: String,
        new_path: String,
        index_files: String,
        parent: String,
        options: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::move_object_file_api(
            self,
            connection,
            object_type as u16,
            &object_name,
            &new_path,
            &index_files,
            &parent,
            options,
        )
    }

    fn create_publication(
        &self,
        connection: Handle,
        name: String,
        comment: String,
        options: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::create_publication_api(
            self, connection, &name, &comment, options,
        )
    }

    fn delete_publication(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::delete_publication_api(self, connection, &name)
    }

    fn set_publication_property(
        &self,
        connection: Handle,
        name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_publication_property_api(
            self,
            connection,
            &name,
            property_id,
            &value,
        )
    }

    fn create_article(
        &self,
        connection: Handle,
        publication: String,
        table: String,
        row_ident: String,
        filter: String,
        options: u32,
        wide: bool,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::create_article_api(
            self,
            connection,
            &publication,
            &table,
            &row_ident,
            &filter,
            options,
            wide,
        )
    }

    fn delete_article(
        &self,
        connection: Handle,
        publication: String,
        table: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::delete_article_api(self, connection, &publication, &table)
    }

    fn set_article_property(
        &self,
        connection: Handle,
        publication: String,
        table: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_article_property_api(
            self,
            connection,
            &publication,
            &table,
            property_id,
            &value,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_subscription(
        &self,
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
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::create_subscription_api(
            self,
            connection,
            &name,
            &publication,
            &target,
            &user,
            &password,
            &queue,
            forward,
            &comment,
            options,
        )
    }

    fn delete_subscription(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::delete_subscription_api(self, connection, &name)
    }

    fn set_subscription_property(
        &self,
        connection: Handle,
        name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_subscription_property_api(
            self,
            connection,
            &name,
            property_id,
            &value,
        )
    }

    fn add_index_file(
        &self,
        connection: Handle,
        table: String,
        path: String,
        comment: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_index_file_api(
            self, connection, &table, &path, &comment,
        )
    }

    fn free_table(&self, path: String, password: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::free_table_api(self, &path, &password)
    }

    fn set_table_property(
        &self,
        connection: Handle,
        table: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_table_property_api(
            self,
            connection,
            &table,
            property_id,
            &value,
        )
    }

    fn set_view_property(
        &self,
        connection: Handle,
        view: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_view_property_api(
            self,
            connection,
            &view,
            property_id,
            &value,
        )
    }

    fn set_field_property(
        &self,
        connection: Handle,
        table: String,
        field: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_field_property_api(
            self,
            connection,
            &table,
            &field,
            property_id,
            &value,
        )
    }

    fn set_index_property(
        &self,
        connection: Handle,
        table: String,
        index: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_index_property_api(
            self,
            connection,
            &table,
            &index,
            property_id,
            &value,
        )
    }

    fn set_database_property(
        &self,
        connection: Handle,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_database_property_api(
            self,
            connection,
            property_id,
            &value,
        )
    }

    fn set_user_property(
        &self,
        connection: Handle,
        user_name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_user_property_api(
            self,
            connection,
            &user_name,
            property_id,
            &value,
        )
    }

    fn set_user_group_property(
        &self,
        connection: Handle,
        group_name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_user_group_property_api(
            self,
            connection,
            &group_name,
            property_id,
            &value,
        )
    }

    fn set_procedure_property(
        &self,
        connection: Handle,
        procedure_name: String,
        property_id: u16,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_procedure_property_api(
            self,
            connection,
            &procedure_name,
            property_id,
            &value,
        )
    }

    fn remove_view(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_view_api(self, connection, &name)
    }

    fn create_group(
        &self,
        connection: Handle,
        name: String,
        description: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::create_group_api(self, connection, &name, &description)
    }

    fn delete_group(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::delete_group_api(self, connection, &name)
    }

    fn create_user(
        &self,
        connection: Handle,
        group: String,
        name: String,
        password: String,
        description: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::create_user_api(
            self,
            connection,
            &group,
            &name,
            &password,
            &description,
        )
    }

    fn delete_user(&self, connection: Handle, name: String) -> Result<(), Error> {
        tflip_engine::dictionary_write::delete_user_api(self, connection, &name)
    }

    fn add_user_to_group(
        &self,
        connection: Handle,
        group: String,
        user: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::add_user_to_group_api(self, connection, &group, &user)
    }

    fn remove_user_from_group(
        &self,
        connection: Handle,
        group: String,
        user: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_user_from_group_api(self, connection, &group, &user)
    }

    fn grant_permission(
        &self,
        connection: Handle,
        object_type: u16,
        object_name: String,
        parent_name: String,
        grantee: String,
        rights: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::grant_permission_api(
            self,
            connection,
            object_type,
            &object_name,
            &parent_name,
            &grantee,
            rights,
        )
    }

    fn revoke_permission(
        &self,
        connection: Handle,
        object_type: u16,
        object_name: String,
        parent_name: String,
        grantee: String,
        rights: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::revoke_permission_api(
            self,
            connection,
            object_type,
            &object_name,
            &parent_name,
            &grantee,
            rights,
        )
    }

    fn set_object_access_rights(
        &self,
        connection: Handle,
        object_name: String,
        accessor_name: String,
        allowed_access: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::set_object_access_rights_api(
            self,
            connection,
            &object_name,
            &accessor_name,
            &allowed_access,
        )
    }

    fn get_permissions(
        &self,
        connection: Handle,
        grantee: String,
        object_type: u16,
        object_name: String,
        parent_name: String,
        inherited: bool,
    ) -> Result<u32, Error> {
        tflip_engine::dictionary_write::get_permissions_api(
            self,
            connection,
            &grantee,
            object_type,
            &object_name,
            &parent_name,
            inherited,
        )
    }

    fn remove_table(
        &self,
        connection: Handle,
        name: String,
        delete_files: u16,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_table_api(self, connection, &name, delete_files != 0)
    }

    fn remove_index_file(
        &self,
        connection: Handle,
        table: String,
        index_file: String,
        delete_file: u16,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::remove_index_file_api(
            self,
            connection,
            &table,
            &index_file,
            delete_file != 0,
        )
    }

    fn dd_delete_index(
        &self,
        connection: Handle,
        table: String,
        index: String,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::delete_index_api(self, connection, &table, &index)
    }

    fn rename_object(
        &self,
        connection: Handle,
        name: String,
        new_name: String,
        object_type: u16,
        options: u32,
    ) -> Result<(), Error> {
        tflip_engine::dictionary_write::rename_object_api(
            self,
            connection,
            &name,
            &new_name,
            object_type,
            options,
        )
    }

    fn restructure_table(
        &self,
        connection: Handle,
        name: String,
        settings: TableSettings,
        add: String,
        delete: String,
        change: String,
        memo_block_size: u32,
    ) -> Result<(), Error> {
        use tflip_engine::restructure::{Changes, restructure_table};
        let changes = Changes::parse(&add, &delete, &change, memo_block_size, settings.table_type)?;
        restructure_table(self, connection, &name, settings, &changes)
    }

    fn create_index(
        &self,
        table: Handle,
        file: Option<String>,
        tag: String,
        expression: String,
        condition: String,
        options: u32,
        page_size: u32,
    ) -> Result<Handle, Error> {
        let file = file.as_deref();
        index::create(
            self,
            table,
            file,
            &tag,
            &expression,
            &condition,
            options,
            page_size,
        )
    }

    fn delete_index(&self, order: Handle) -> Result<(), Error> {
        index::delete(self, order)
    }

    fn reindex(&self, handle: Handle) -> Result<(), Error> {
        index::reindex(self, handle)
    }

    fn create_fts_index(
        &self,
        table: Handle,
        file: Option<String>,
        tag: String,
        field: String,
        page_size: u32,
        min_word: u32,
        max_word: u32,
        lists: Vec<(bool, Option<String>)>,
        options: u32,
    ) -> Result<(), Error> {
        let block = index::full_text_block(min_word, max_word, &lists, options)?;
        index::create_full_text(self, table, file.as_deref(), &tag, &field, page_size, block)
    }

    fn reindex_fts(&self, table: Handle, page_size: u32) -> Result<(), Error> {
        index::reindex_fts(self, table, page_size)
    }

    fn reindex_pages(&self, handle: Handle, page_size: u32) -> Result<(), Error> {
        index::reindex_pages(
            self,
            handle,
            Some(page_size as u16).filter(|&size| size != 0),
        )
    }

    fn pack_table(&self, table: Handle, zap: bool, memo_block_size: u32) -> Result<(), Error> {
        index::pack_to(self, table, zap, memo_block_size)
    }

    fn copy_table(
        &self,
        handle: Handle,
        filter: u16,
        name: String,
        structure_only: bool,
    ) -> Result<(), Error> {
        tflip_engine::copy::copy_table(self, handle, filter, &name, structure_only)
    }

    fn convert_table(
        &self,
        handle: Handle,
        filter: u16,
        name: String,
        table_type: tflip_engine::table::TableType,
    ) -> Result<(), Error> {
        tflip_engine::copy::convert_table(self, handle, filter, &name, table_type)
    }

    fn copy_contents(&self, from: Handle, to: Handle, filter: u16) -> Result<(), Error> {
        tflip_engine::copy::copy_contents(self, from, to, filter)
    }

    fn clone_table(&self, table: Handle) -> Result<Handle, Error> {
        table::clone_table(self, table)
    }

    fn recall_all(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.recall_all()
    }

    fn delete_table(&self, table: Handle) -> Result<(), Error> {
        table::delete(self, table)
    }

    fn set_transaction_free(&self, table: Handle, free: bool) -> Result<(), Error> {
        table::get(self, table)?.set_transaction_free(self, free)
    }

    fn enable_encryption(&self, table: Handle, password: String) -> Result<(), Error> {
        table::get(self, table)?.enable_encryption(password.as_bytes())
    }

    fn disable_encryption(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.disable_encryption()
    }

    fn encrypt_table(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.encrypt_table()
    }

    fn decrypt_table(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.decrypt_table()
    }

    fn encrypt_record(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.encrypt_record()
    }

    fn decrypt_record(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.decrypt_record()
    }

    fn encryption_enabled(&self, table: Handle) -> Result<bool, Error> {
        Ok(table::get(self, table)?.encryption_enabled())
    }

    fn record_encrypted(&self, table: Handle) -> Result<bool, Error> {
        table::get(self, table)?.record_encrypted()
    }

    fn close_all_indexes(&self, table: Handle) -> Result<(), Error> {
        index::close_files(self, table)
    }

    fn index_by_name(&self, table: Handle, name: String) -> Result<Handle, Error> {
        index::by_name(self, table, &name)
    }

    fn expression_to_short(&self, table: Handle, expression: String) -> Result<String, Error> {
        Ok(table::get(self, table)?.with_field_numbers(&expression))
    }

    fn expression_to_long(&self, table: Handle, expression: String) -> Result<String, Error> {
        table::get(self, table)?.with_field_names(&expression)
    }

    fn key_filter(
        &self,
        table: Handle,
        values_table: String,
        options: u32,
    ) -> Result<String, Error> {
        let values = (!values_table.is_empty()).then_some(values_table.as_str());
        tflip_engine::index::key_filter(self, table, values, options)
    }

    fn goto_top(&self, table: Handle) -> Result<(), Error> {
        moving(self, table, Table::goto_top, Table::goto_top_in)
    }

    fn goto_bottom(&self, table: Handle) -> Result<(), Error> {
        moving(self, table, Table::goto_bottom, Table::goto_bottom_in)
    }

    fn goto_bof(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.goto_bof()
    }

    fn goto_eof(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.goto_eof()
    }

    fn skip(&self, table: Handle, records: i64) -> Result<(), Error> {
        moving(
            self,
            table,
            |opened| opened.skip(records),
            |opened, order| opened.skip_in(order, records),
        )
    }

    fn goto_record(&self, table: Handle, record: u32) -> Result<(), Error> {
        table::get(self, table)?.goto_record(record)
    }

    fn position(&self, table: Handle) -> Result<Position, Error> {
        Ok(table::get(self, table)?.position())
    }

    fn record_number(&self, table: Handle, visible: bool) -> Result<u32, Error> {
        let opened = table::get(self, table)?;
        let number = opened.record_number_checked()?;
        if visible {
            opened.visible_record_number()
        } else {
            Ok(number)
        }
    }

    fn found(&self, table: Handle) -> Result<bool, Error> {
        Ok(table::get(self, table)?.found())
    }

    fn is_visible(&self, table: Handle) -> Result<bool, Error> {
        table::get(self, table)?.is_visible()
    }

    fn is_deleted(&self, table: Handle) -> Result<bool, Error> {
        table::get(self, table)?.is_deleted()
    }

    fn read_record(&self, table: Handle) -> Result<Vec<u8>, Error> {
        table::get(self, table)?.shown_record()
    }

    fn read_fields(
        &self,
        table: Handle,
        want: Want,
    ) -> Result<Vec<Option<Result<Scalar, Error>>>, Error> {
        let table = table::get(self, table)?;
        Ok(table
            .fields()
            .iter()
            .enumerate()
            .map(|(i, field)| {
                getters::row_cacheable(field)
                    .then(|| getters::field_as(&table, &FieldId::Number(i + 1), want))
            })
            .collect())
    }

    fn value(&self, table: Handle, field: usize) -> Result<Value, Error> {
        table::get(self, table)?.value(FieldRef::Number(field))
    }

    fn is_null(&self, table: Handle, field: usize) -> Result<bool, Error> {
        table::get(self, table)?.is_null(FieldRef::Number(field))
    }

    fn is_empty(&self, table: Handle, field: usize) -> Result<bool, Error> {
        table::get(self, table)?.is_empty(FieldRef::Number(field))
    }

    fn memo_length(&self, table: Handle, field: usize) -> Result<u32, Error> {
        table::get(self, table)?.memo_length(FieldRef::Number(field))
    }

    fn read_memo(&self, table: Handle, field: usize) -> Result<Vec<u8>, Error> {
        table::get(self, table)?.read_memo(FieldRef::Number(field))
    }

    fn data_length(&self, table: Handle, field: usize, unit: LengthUnit) -> Result<u32, Error> {
        table::get(self, table)?.data_length(FieldRef::Number(field), unit)
    }

    fn index_info(&self, order: Handle) -> Result<IndexInfo, Error> {
        Ok(index::get(self, order)?.info())
    }

    fn key_number(&self, order: Handle, scoped: bool) -> Result<u32, Error> {
        in_order(self, order, |table, order| {
            table.key_number_in(order, scoped)
        })
    }

    fn key_count(&self, order: Handle, scoped: bool) -> Result<u32, Error> {
        in_order(self, order, |table, order| {
            table.key_count_in(order, scoped)
        })
    }

    fn relative_position(&self, order: Handle) -> Result<f64, Error> {
        in_order(self, order, |table, order| {
            table.relative_position_in(order)
        })
    }

    fn set_relative_position(&self, order: Handle, position: f64) -> Result<(), Error> {
        in_order(self, order, |table, order| {
            table.set_relative_position_in(order, position)
        })
    }

    fn seek(&self, order: Handle, key: Key, how: SeekType) -> Result<bool, Error> {
        in_order(self, order, |table, order| {
            table.seek_in(order, key.as_seek_value(), how)
        })
    }

    fn seek_last(&self, order: Handle, key: Key) -> Result<bool, Error> {
        in_order(self, order, |table, order| {
            table.seek_last_in(order, key.as_seek_value())
        })
    }

    fn skip_unique(&self, order: Handle, records: i64) -> Result<(), Error> {
        in_order(self, order, |table, order| {
            table.skip_unique_in(order, records)
        })
    }

    fn extract_key(&self, order: Handle) -> Result<Vec<u8>, Error> {
        in_order(self, order, |table, order| table.current_key(order))
    }

    fn bookmark_key(&self, order: Handle) -> Result<Vec<u8>, Error> {
        in_order(self, order, |table, order| table.bookmark_key(order))
    }

    fn lookup_key(&self, order: Handle, key: Key) -> Result<bool, Error> {
        in_order(self, order, |table, order| {
            table.lookup_key(order, key.as_seek_value())
        })
    }

    fn key_from_record(&self, order: Handle, record: Vec<u8>) -> Result<Vec<u8>, Error> {
        in_order(self, order, |table, order| {
            table.key_from_record(order, &record)
        })
    }

    fn init_raw_key(&self, order: Handle) -> Result<(), Error> {
        index::get(self, order)?.init_raw_key();
        Ok(())
    }

    fn set_key_field(&self, order: Handle, field: FieldId, value: SetValue) -> Result<(), Error> {
        index::get(self, order)?.set_raw_key_field(field.as_ref(), &value)
    }

    fn build_raw_key(&self, order: Handle, options: u32) -> Result<Vec<u8>, Error> {
        index::get(self, order)?.build_raw_key(options)
    }

    fn add_custom_key(&self, order: Handle) -> Result<(), Error> {
        in_order(self, order, |table, order| table.add_custom_key(order))
    }

    fn delete_custom_key(&self, order: Handle) -> Result<(), Error> {
        in_order(self, order, |table, order| table.delete_custom_key(order))
    }

    fn set_index_direction(&self, order: Handle, reversed: bool) -> Result<(), Error> {
        index::get(self, order)?.set_reversed(reversed);
        Ok(())
    }

    fn set_scope(&self, order: Handle, bottom: bool, key: Key) -> Result<(), Error> {
        in_order(self, order, |table, order| {
            table.set_scope(order, bottom, key.as_seek_value())
        })
    }

    fn scope_key(&self, order: Handle, bottom: bool) -> Result<Vec<u8>, Error> {
        index::get(self, order)?.scope_key(bottom)
    }

    fn clear_scope(&self, order: Handle, bottom: bool) -> Result<(), Error> {
        index::get(self, order)?.clear_scope(bottom)
    }

    fn clear_all_scopes(&self, table: Handle) -> Result<(), Error> {
        index::clear_all_scopes(self, table)
    }

    fn set_aof(&self, table: Handle, expression: String, options: AofOptions) -> Result<(), Error> {
        let opened = table::get(self, table)?;
        opened.set_aof(&expression, options, &index::open_on(self, table))
    }

    fn aof(&self, table: Handle) -> Result<String, Error> {
        table::get(self, table)?.aof()
    }

    fn aof_level(&self, table: Handle) -> Result<(u16, String), Error> {
        let (level, residual) = table::get(self, table)?.aof_level()?;
        Ok((level.ace(), residual))
    }

    fn eval_aof(&self, table: Handle, expression: String) -> Result<u16, Error> {
        let opened = table::get(self, table)?;
        Ok(opened
            .optimize(&expression, &index::open_on(self, table))?
            .level
            .ace())
    }

    fn clear_aof(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.clear_aof()
    }

    fn set_aof_active(&self, table: Handle, active: bool) -> Result<(), Error> {
        table::get(self, table)?.set_aof_active(active)
    }

    fn refresh_aof(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.refresh_aof(&index::open_on(self, table))
    }

    fn customize_aof(&self, table: Handle, records: Vec<u32>, how: Customize) -> Result<(), Error> {
        table::get(self, table)?.customize_aof(&records, how)
    }

    fn is_in_aof(&self, table: Handle, record: u32) -> Result<bool, Error> {
        table::get(self, table)?.is_in_aof(record)
    }

    fn set_filter(&self, table: Handle, expression: String) -> Result<(), Error> {
        table::get(self, table)?.set_filter(&expression)
    }

    fn filter(&self, table: Handle) -> Result<String, Error> {
        table::get(self, table)?.filter()
    }

    fn clear_filter(&self, table: Handle) -> Result<(), Error> {
        table::get(self, table)?.clear_filter()
    }

    fn check_expression(&self, table: Handle, expression: String) -> Result<(), Error> {
        table::get(self, table)?.compile(&expression).map(|_| ())
    }

    fn eval(&self, table: Handle, expression: String) -> Result<Evaluation, Error> {
        table::get(self, table)?.eval(&expression)
    }

    fn locate(&self, table: Handle, expression: String, forward: bool) -> Result<bool, Error> {
        table::get(self, table)?.locate(&expression, forward)
    }

    fn continue_locate(&self, table: Handle) -> Result<bool, Error> {
        table::get(self, table)?.continue_locate()
    }

    fn create_statement(&self, connection: Handle) -> Result<Handle, Error> {
        statement::create(self, connection)
    }

    fn close_statement(&self, statement: Handle) -> Result<(), Error> {
        statement::close(self, statement)
    }

    fn prepare(&self, statement: Handle, sql: String) -> Result<(), Error> {
        statement::prepare(self, statement, &sql)
    }

    fn verify(&self, statement: Handle, sql: String) -> Result<(), Error> {
        statement::verify(self, statement, &sql)
    }

    fn execute(&self, statement: Handle, want_cursor: bool) -> Result<Option<Handle>, Error> {
        statement::execute(self, statement, want_cursor)
    }

    fn execute_direct(
        &self,
        statement: Handle,
        sql: String,
        want_cursor: bool,
    ) -> Result<Option<Handle>, Error> {
        statement::execute_direct(self, statement, &sql, want_cursor)
    }

    fn set_param(
        &self,
        statement: Handle,
        param: ParamRef,
        value: ParamValue,
    ) -> Result<(), Error> {
        statement::set_param(self, statement, param, value)
    }

    fn set_relation(
        &self,
        parent: Handle,
        order: Handle,
        expression: String,
        scoped: bool,
    ) -> Result<(), Error> {
        tflip_engine::relation::set(self, parent, order, &expression, scoped)
    }

    fn clear_relation(&self, parent: Handle) -> Result<(), Error> {
        tflip_engine::relation::clear(self, parent)
    }

    fn begin_transaction(&self, connection: Handle) -> Result<(), Error> {
        transaction::begin(self, connection)
    }

    fn commit_transaction(&self, connection: Handle) -> Result<(), Error> {
        transaction::api_commit(self, connection)
    }

    fn rollback_transaction(
        &self,
        connection: Handle,
        savepoint: Option<String>,
    ) -> Result<(), Error> {
        transaction::api_rollback(self, connection, savepoint.as_deref())
    }

    fn create_savepoint(&self, connection: Handle, name: String) -> Result<(), Error> {
        transaction::savepoint(self, connection, &name)
    }

    fn transaction_count(&self, connection: Handle) -> Result<u32, Error> {
        transaction::count(self, connection)
    }

    fn in_transaction(&self, connection: Handle) -> Result<bool, Error> {
        transaction::api_active(self, connection)
    }

    fn recover_transactions(&self) -> Result<u32, Error> {
        tflip_engine::txlog::recover(self).map(|n| n as u32)
    }

    fn clear_params(&self, statement: Handle) -> Result<(), Error> {
        statement::clear_params(self, statement)
    }

    fn set_statement(&self, statement: Handle, setting: StatementSetting) -> Result<(), Error> {
        statement::set(self, statement, setting)
    }

    fn statement_text(&self, statement: Handle) -> Result<String, Error> {
        statement::text(self, statement)
    }

    fn statement_params(&self, statement: Handle) -> Result<u32, Error> {
        statement::param_total(self, statement)
    }

    fn affected(&self, statement: Handle) -> Result<u32, Error> {
        statement::affected(self, statement)
    }

    fn cursor_statement(&self, cursor: Handle) -> Result<Handle, Error> {
        statement::of_cursor(self, cursor)
    }

    fn open_cursor(&self, statement: Handle) -> Result<Option<Handle>, Error> {
        statement::open_cursor(self, statement)
    }

    fn cursor_is_live(&self, cursor: Handle) -> Result<bool, Error> {
        statement::is_live(self, cursor)
    }
}
