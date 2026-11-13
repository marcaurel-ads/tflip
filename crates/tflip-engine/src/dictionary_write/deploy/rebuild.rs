//! The dictionary a deployment leaves in the destination. The original does
//! not copy the source's files: it makes a new dictionary and adds the
//! source's objects to it, tables first, then views, links, procedures,
//! triggers, referential integrity rules, publications, subscriptions,
//! packages, functions, groups and users, each group and user followed by the
//! rights granted to it, so every object takes a new id and every record that
//! names another object, or is enciphered with its own id, is written again
//! (`fixtures/dictionary/deploy`, `x_*`).

use std::collections::{HashMap, HashSet};

use tflip_storage::crypto;
use tflip_storage::dd::{self, Dictionary, Kind, Object, create::NewObject};

use super::*;

/// Whether the kind of object takes its rights granted to `DB:Public` to the
/// deployed dictionary: a view's, a procedure's, a link's and a publication's
/// are granted anew, while a function's, a package's, a user's, a group's and
/// the database's are left behind (`fixtures/dictionary/deploy/x_pubm`,
/// `x_pubk` and the others of its kind).
fn keeps_public_rights(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::View | Kind::Procedure | Kind::Link | Kind::Publication
    )
}

/// The source's objects in the order the deployment adds them, and the id
/// each takes.
pub(super) struct Plan<'a> {
    order: Vec<&'a Object>,
    ids: HashMap<u32, u32>,
    /// Whether a table's rights granted to `DB:Public` stop the deployment:
    /// the original grants them once the groups are added, answers 5136, and
    /// leaves what it has made (`fixtures/dictionary/deploy/x_pubt*`). The
    /// order then ends before the users.
    pub(super) fails_on_public: bool,
}

impl Plan<'_> {
    /// The id the source's object `id` takes.
    pub(super) fn id(&self, id: u32) -> Option<u32> {
        self.ids.get(&id).copied()
    }
}

/// The queue table of `subscription`: the `__<name>` table made with it, the
/// object after it.
pub(super) fn queue_table<'a>(dict: &'a Dictionary, subscription: &Object) -> Option<&'a Object> {
    dict.object(subscription.id + 1).filter(|table| {
        table.kind == Kind::Table
            && table
                .name
                .eq_ignore_ascii_case(&format!("__{}", subscription.name))
    })
}

/// The tables the deployment looks for in the destination: those of the
/// dictionary but its subscriptions' queues, which are made with the
/// subscriptions (`fixtures/dictionary/deploy/x_sub`).
pub(super) fn tables(dict: &Dictionary) -> Vec<&Object> {
    let queues: HashSet<u32> = dict
        .all(Kind::Subscription)
        .iter()
        .filter_map(|subscription| queue_table(dict, subscription))
        .map(|table| table.id)
        .collect();
    dict.all(Kind::Table)
        .into_iter()
        .filter(|table| !queues.contains(&table.id))
        .collect()
}

/// Adds `table` to `order`, then its fields, then each index file with its
/// orders.
fn with_children<'a>(dict: &'a Dictionary, order: &mut Vec<&'a Object>, table: &'a Object) {
    order.push(table);
    // The fields are added in the order they are stored in, not their own.
    let mut fields = dict.children(table.id, Kind::Field);
    fields.sort_by_key(|field| field.record());
    order.extend(fields);
    let keys = dict.children(table.id, Kind::Index);
    for file in dict.children(table.id, Kind::IndexFile) {
        order.push(file);
        order.extend(keys.iter().filter(|key| key.info1 == Some(file.id as i32)));
    }
}

/// Whether the permission still grants a right: a revoked one stays in the
/// dictionary with none, and is not added to the new one.
fn grants_anything(dict: &Dictionary, key: &[u8], permission: &Object) -> bool {
    (dd::permission::OBJECT_RIGHTS..=dd::permission::WITH_GRANT_COLUMN_RIGHTS).any(|slot| {
        let Some(bytes) = permission.slot(slot) else {
            return false;
        };
        let mut blob = bytes.to_vec();
        let item = crypto::permission_item(permission.id, slot as u32);
        crypto::crypt(dict.scheme(), key, item, &mut blob);
        match crypto::permission_rights(&blob) {
            Some(rights) => rights != 0,
            None => crypto::permission_column_rights(&blob)
                .is_none_or(|columns| columns.iter().any(|&column| column != 0)),
        }
    })
}

/// Orders the objects of `dict`, whose rights are enciphered with `key`, or
/// refuses a dictionary with ones the deployment cannot add as the original
/// does.
pub(super) fn plan<'a>(dict: &'a Dictionary, key: &[u8]) -> Result<Plan<'a>, Error> {
    let database = dict.database().ok_or(Error::CorruptTable)?;
    let mut order: Vec<&Object> = vec![database];
    for table in tables(dict) {
        with_children(dict, &mut order, table);
    }
    for kind in [
        Kind::View,
        Kind::Link,
        Kind::Procedure,
        Kind::Trigger,
        Kind::Relation,
    ] {
        order.extend(dict.all(kind));
    }
    for publication in dict.all(Kind::Publication) {
        order.push(publication);
        order.extend(dict.children(publication.id, Kind::Article));
    }
    // Each subscription brings the queue table it was made with.
    for subscription in dict.all(Kind::Subscription) {
        order.push(subscription);
        if let Some(queue) = queue_table(dict, subscription) {
            with_children(dict, &mut order, queue);
        }
    }
    let packages = dict.all(Kind::Package);
    order.extend(&packages);
    let functions = dict.all(Kind::Function);
    order.extend(
        functions
            .iter()
            .filter(|f| dict.object(f.parent) == Some(database)),
    );
    for package in &packages {
        order.extend(functions.iter().filter(|f| f.parent == package.id));
    }
    let permissions: Vec<&Object> = dict
        .all(Kind::Permission)
        .into_iter()
        .filter(|permission| grants_anything(dict, key, permission))
        .collect();
    for kind in [Kind::Group, Kind::User] {
        for grantee in dict.all(kind) {
            if kind == Kind::Group && dd::built_in_group(&grantee.name).is_some() {
                return Err(Error::NotImplemented);
            }
            order.push(grantee);
            order.extend(permissions.iter().filter(|p| p.parent == grantee.id));
        }
    }

    // What no kind above places is one the original treats in a way not
    // known.
    let revoked = dict.all(Kind::Permission).len() != permissions.len();
    let placed: HashSet<u32> = order.iter().map(|object| object.id).collect();
    let all_placed = dict
        .objects()
        .iter()
        .all(|o| placed.contains(&o.id) || (revoked && o.kind == Kind::Permission));
    let unique = placed.len() == order.len();
    // A table with none of its rights left has none to grant.
    let mut invalid = false;
    let mut fails_on_public = false;
    for object in dict.objects() {
        match dict.public_rights(object, Some(key)) {
            dd::PublicRights::Absent => {}
            dd::PublicRights::Rights(rights) => {
                fails_on_public |= object.kind == Kind::Table && rights != 0
            }
            dd::PublicRights::Invalid => invalid = true,
        }
    }
    // The original keeps a field's message only sometimes, and then drops
    // other properties of the fields, by a rule not worked out.
    let messages = dict
        .objects()
        .iter()
        .any(|object| object.kind == Kind::Field && object.has(dd::field::VALIDATION_MSG));
    if !all_placed || !unique || invalid || messages {
        return Err(Error::NotImplemented);
    }
    if fails_on_public && let Some(first) = order.iter().position(|o| o.kind == Kind::User) {
        order.truncate(first);
    }
    let ids = order
        .iter()
        .enumerate()
        .map(|(n, object)| (object.id, n as u32 + 1))
        .collect();
    Ok(Plan {
        order,
        ids,
        fails_on_public,
    })
}

/// Whether the deployed database requires logins: the source's marker of
/// logins not required goes unless the security level was set.
pub(super) fn logins_required(database: &Object) -> bool {
    !database.has(dd::database::LOGIN_NOT_REQUIRED)
        || !database.has(dd::database::INTERNET_SECURITY_LEVEL)
}

/// The dictionary's version the objects it holds need: users and rights 3 (the
/// rights granted to `DB:Public` that go to it too), triggers 4.
fn version(dict: &Dictionary, key: &[u8]) -> (u32, &'static [u8]) {
    let public = dict.objects().iter().any(|object| {
        (keeps_public_rights(object.kind) || object.kind == Kind::Table)
            && matches!(
                dict.public_rights(object, Some(key)),
                dd::PublicRights::Rights(rights) if rights != 0
            )
    });
    if !dict.all(Kind::Trigger).is_empty() {
        (4, b"8.1\0")
    } else if !dict.all(Kind::User).is_empty()
        || public
        || dict
            .all(Kind::Permission)
            .iter()
            .any(|permission| grants_anything(dict, key, permission))
    {
        (3, b"7.1\0")
    } else {
        (2, b"6.2\0")
    }
}

/// `target`, the path of the dictionary a folder keeps, as seen from `folder`
/// (`..\S\x.add` for the file of a sibling folder).
fn relative_to(folder: &Path, target: &Path) -> String {
    let mut ups = folder.components().peekable();
    let mut down = target.components().peekable();
    while let (Some(a), Some(b)) = (ups.peek(), down.peek()) {
        if a != b {
            break;
        }
        ups.next();
        down.next();
    }
    let up = ups.count();
    let mut text = String::new();
    if up == 0 {
        text.push_str(".\\");
    }
    text.push_str(&"..\\".repeat(up));
    let rest: Vec<String> = down
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    text.push_str(&rest.join("\\"));
    text
}

/// Resolves `.` and `..` of a path without looking at the disk.
fn normalised(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Takes `text` as the trigger's script stores it: without the white space
/// around it, before its terminating NUL.
fn trimmed(text: &[u8]) -> Vec<u8> {
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    let mut kept = text[..end].trim_ascii().to_vec();
    kept.push(0);
    kept
}

/// What a deployment writes of an object `old` of the source as its new
/// record `record`, with the id `id`.
struct Writer<'a> {
    dict: &'a Dictionary,
    plan: &'a Plan<'a>,
    key: Vec<u8>,
    scheme: crypto::Scheme,
    source_folder: &'a Path,
    folder: &'a Path,
}

impl Writer<'_> {
    /// The id the source's object `id` takes, if it is one the deployment
    /// knows.
    fn map(&self, id: u32) -> Result<u32, Error> {
        self.plan.id(id).ok_or(Error::NotImplemented)
    }

    fn map_u32(&self, slot: &mut Vec<u8>) -> Result<(), Error> {
        let old = u32::from_le_bytes(
            slot.get(..4)
                .ok_or(Error::CorruptTable)?
                .try_into()
                .unwrap(),
        );
        *slot = self.map(old)?.to_le_bytes().to_vec();
        Ok(())
    }

    /// Enciphers `data` again for the item `new` it was enciphered for at
    /// `old`.
    fn recrypt(&self, data: &mut [u8], old: u32, new: u32) {
        crypto::crypt(self.scheme, &self.key, old, data);
        crypto::crypt(self.scheme, &self.key, new, data);
    }
}

/// Writes the deployed dictionary at `target` from `dict`, read from
/// `source` and enciphered with `key`, with the objects as `plan` orders
/// them. The database's login secret is the source's, so the deployed
/// dictionary has the same administrator password
/// (`fixtures/dictionary/deploy/src_pw_given`).
pub(super) fn write(
    session: &Session,
    dict: &Dictionary,
    key: Vec<u8>,
    plan: &Plan,
    source: &Path,
    target: &Path,
    freed: &[&Object],
) -> Result<(), Error> {
    use dd::create;

    let memo_path = dd::memo_path(target);
    let base = create::new_dictionary_add(&login_salts()).map_err(|_| Error::FileCreate)?;
    std::fs::write(&memo_path, create::new_dictionary_memo()).map_err(|_| Error::FileCreate)?;
    std::fs::write(target, &base).map_err(|_| Error::FileCreate)?;
    let mut file = dd::DictionaryFile::open(target).map_err(|_| Error::FileCreate)?;

    let writer = Writer {
        dict,
        plan,
        key,
        scheme: dict.scheme(),
        source_folder: source.parent().unwrap_or_else(|| Path::new(".")),
        folder: target.parent().unwrap_or_else(|| Path::new(".")),
    };
    for (n, old) in plan.order.iter().enumerate() {
        let id = n as u32 + 1;
        let record = writer.object(old, id, source, &memo_path)?;
        match n {
            0 => file.write(1, &record).map_err(|_| Error::CorruptTable)?,
            _ => drop(file.insert(&record).map_err(|_| Error::CorruptTable)?),
        }
    }
    file.set_next_autoinc(plan.order.len() as u32 + 1)
        .map_err(|_| Error::CorruptTable)?;
    drop(file);
    renumber_ri_graphs(target)?;

    // The tables the original frees again leave their records deleted.
    if !freed.is_empty() {
        let made = Dictionary::open(target).map_err(|_| Error::CorruptTable)?;
        let mut file = dd::DictionaryFile::open(target).map_err(|_| Error::CorruptTable)?;
        for old in freed {
            let id = plan.id(old.id).ok_or(Error::CorruptTable)?;
            let table = made.object(id).ok_or(Error::CorruptTable)?;
            let mut slots = super::super::removal::table_slots(&made, table);
            // The articles that name the table go with it.
            slots.extend(
                made.all(Kind::Article)
                    .iter()
                    .filter(|article| article.info1 == i32::try_from(id).ok())
                    .map(|article| article.record()),
            );
            for slot in slots {
                create::free_object_memos(target, slot).map_err(|_| Error::CorruptTable)?;
                file.delete(slot, None).map_err(|_| Error::CorruptTable)?;
            }
        }
    }

    let table = Arc::new(table::dictionary(session, target)?);
    let weights = table
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = tflip_storage::create::collation_crc(weights.as_ref());
    let ai_path = target.with_extension("ai");
    std::fs::remove_file(&ai_path).ok();
    index::build_dictionary_index(&table, &ai_path, collation)
}

impl Writer<'_> {
    /// The record of the source's object `old` as the deployed dictionary
    /// has it, with the id `id`.
    fn object(
        &self,
        old: &Object,
        id: u32,
        source: &Path,
        memo_path: &Path,
    ) -> Result<Vec<u8>, Error> {
        let dict = self.dict;
        let parent = match old.kind {
            Kind::Database => 0,
            _ => self.map(old.parent)? as i32,
        };
        let mut slots: Vec<(usize, Vec<u8>)> = old
            .slots()
            .map(|(slot, bytes)| (slot, bytes.to_vec()))
            .collect();
        let mut name = old.name.clone();
        let mut info1 = old.info1;
        let info2 = old.info2.map_or(0x8000_0000, |n| n as u32);
        let slot_mut = |slots: &mut Vec<(usize, Vec<u8>)>, slot: usize| {
            slots
                .iter()
                .position(|(s, _)| *s == slot)
                .map(|at| slots.remove(at).1)
        };
        let put = |slots: &mut Vec<(usize, Vec<u8>)>, slot: usize, value: Vec<u8>| {
            slots.retain(|(s, _)| *s != slot);
            slots.push((slot, value));
        };
        match old.kind {
            Kind::Database => {
                let (number, text) = version(dict, &self.key);
                put(
                    &mut slots,
                    dd::database::VERSION,
                    number.to_le_bytes().to_vec(),
                );
                put(&mut slots, 17, text.to_vec());
                info1 = Some(number as i32);
                // A setting the source does not have is made, with the
                // logins required unless a security level was set.
                let had = |slot| old.has(slot);
                if logins_required(old) {
                    slot_mut(&mut slots, dd::database::LOGIN_NOT_REQUIRED);
                }
                for (slot, value) in [
                    (dd::database::ENABLE_INTERNET, vec![0, 0]),
                    (dd::database::INTERNET_SECURITY_LEVEL, vec![2, 0]),
                    (dd::database::MAX_FAILED_ATTEMPTS, vec![5, 0]),
                    (10, vec![0, 0]),
                    (dd::database::VERSION_MAJOR, vec![0, 0]),
                    (dd::database::VERSION_MINOR, vec![0, 0]),
                    (dd::database::LOGINS_DISABLED, vec![0, 0]),
                    (dd::database::QUERY_VIA_ROOT, vec![0; 4]),
                    (dd::database::ENFORCE_MAX_FAILED_LOGINS, vec![0, 0]),
                ] {
                    if !had(slot) {
                        put(&mut slots, slot, value);
                    }
                }
                // The deployment that stops has made the settings of its
                // start only: the encryptions and the query via root, not
                // the logins, the rights or the internet's.
                if self.plan.fails_on_public {
                    use dd::database as db;
                    let kept = [
                        0,
                        1,
                        db::ENCRYPT_NEW_TABLE,
                        db::ENCRYPT_INDEXES,
                        db::ENCRYPT_COMMUNICATION,
                        db::VERSION,
                        17,
                        28,
                        db::QUERY_VIA_ROOT,
                    ];
                    slots.retain(|(slot, _)| kept.contains(slot));
                }
            }
            Kind::Table => {
                if crate::dictionary::table_type(old) == TableType::Adt
                    && let Some(key) = old.info1.filter(|&n| n > 0)
                {
                    info1 = Some(self.map(key as u32)? as i32);
                }
                if let Some((_, value)) =
                    slots.iter_mut().find(|(s, _)| *s == dd::table::PRIMARY_KEY)
                {
                    self.map_u32(value)?;
                }
                // What the triggers of the table need is made with them.
                if dict.all(Kind::Trigger).iter().all(|t| t.parent != old.id) {
                    slot_mut(&mut slots, dd::table::TRIGGER_TYPES);
                    slot_mut(&mut slots, dd::table::TRIGGER_OPTIONS);
                }
            }
            Kind::IndexFile => {
                let keys = dict
                    .children(old.parent, Kind::Index)
                    .iter()
                    .filter(|key| key.info1 == Some(old.id as i32))
                    .count();
                // The index file of a subscription's queue keeps what it was
                // made with, not the count of its orders
                // (`fixtures/dictionary/deploy/x_sub`).
                let queue = dict.all(Kind::Subscription).iter().any(|subscription| {
                    queue_table(dict, subscription).is_some_and(|table| table.id == old.parent)
                });
                if old.has(1) && !queue {
                    put(&mut slots, 1, (keys as u16).to_le_bytes().to_vec());
                }
            }
            Kind::Index => {
                if let Some(file) = old.info1 {
                    info1 = Some(self.map(file as u32)? as i32);
                }
            }
            Kind::Trigger => {
                if let Some(table) = old.info1 {
                    info1 = Some(self.map(table as u32)? as i32);
                }
                if let Some((_, script)) =
                    slots.iter_mut().find(|(s, _)| *s == dd::trigger::CONTAINER)
                {
                    *script = trimmed(script);
                }
            }
            Kind::Relation => {
                for slot in [
                    dd::relation::PRIMARY_TABLE,
                    dd::relation::PRIMARY_INDEX,
                    dd::relation::FOREIGN_TABLE,
                    dd::relation::FOREIGN_INDEX,
                ] {
                    if let Some((_, value)) = slots.iter_mut().find(|(s, _)| *s == slot) {
                        self.map_u32(value)?;
                    }
                }
                info1 = Some(id as i32);
            }
            Kind::Article | Kind::Permission => {
                let target = old.info1.ok_or(Error::CorruptTable)? as u32;
                let target = self.map(target)?;
                info1 = Some(target as i32);
                name = format!("{target:08X}");
                if old.kind == Kind::Permission {
                    for slot in
                        dd::permission::OBJECT_RIGHTS..=dd::permission::WITH_GRANT_COLUMN_RIGHTS
                    {
                        if let Some((_, value)) = slots.iter_mut().find(|(s, _)| *s == slot) {
                            self.recrypt(
                                value,
                                crypto::permission_item(old.id, slot as u32),
                                crypto::permission_item(id, slot as u32),
                            );
                        }
                    }
                }
            }
            Kind::Link => {
                if let Some((_, value)) = slots
                    .iter_mut()
                    .find(|(s, _)| *s == dd::link::RELATIVE_PATH)
                {
                    let text = String::from_utf8_lossy(&value[..value.len().saturating_sub(1)])
                        .into_owned();
                    let target = under_folder(self.source_folder, &text);
                    let relative = match Path::new(&text).is_absolute() || text.contains(':') {
                        true => text,
                        false => relative_to(&normalised(self.folder), &normalised(&target)),
                    };
                    *value = nul_bytes(&relative);
                }
            }
            Kind::User | Kind::Group => {
                if let Some((_, value)) = slots
                    .iter_mut()
                    .find(|(s, _)| *s == dd::permission::CREATE_RIGHTS)
                {
                    self.recrypt(
                        value,
                        crypto::permission_item(old.id, dd::permission::CREATE_RIGHTS as u32),
                        crypto::permission_item(id, dd::permission::CREATE_RIGHTS as u32),
                    );
                }
                if old.kind == Kind::User {
                    if let Some((_, value)) = slots
                        .iter_mut()
                        .find(|(s, _)| *s == dd::user::PASSWORD_SECRET)
                    {
                        crypto::obfuscate(self.scheme, value, old.record(), 0);
                        crypto::obfuscate(self.scheme, value, id, 0);
                    }
                    if let Some((_, value)) = slots
                        .iter_mut()
                        .find(|(s, _)| *s == dd::user::GROUP_MEMBERSHIP)
                    {
                        crypto::group_membership(self.scheme, &self.key, value);
                        // The users are put in the groups group by group, so
                        // each one's list is in the groups' order.
                        let mut groups = Vec::new();
                        for group in value.as_chunks::<4>().0 {
                            groups.push(self.map(u32::from_le_bytes(*group))?);
                        }
                        groups.sort_unstable();
                        let mut renumbered: Vec<u8> = groups
                            .iter()
                            .flat_map(|group| group.to_le_bytes())
                            .collect();
                        crypto::group_membership(self.scheme, &self.key, &mut renumbered);
                        *value = renumbered;
                    }
                    // The built-in groups a user was in are not put back.
                    slot_mut(&mut slots, dd::user::BUILT_IN_MEMBERSHIP);
                }
            }
            Kind::Subscription => {
                let publication = old.info1.ok_or(Error::CorruptTable)? as u32;
                info1 = Some(self.map(publication)? as i32);
                // The password and the connection string are enciphered with
                // the subscription's own record.
                for slot in [dd::subscription::PASSWORD, dd::subscription::CONNECTION_STR] {
                    if let Some((_, value)) = slots.iter_mut().find(|(s, _)| *s == slot) {
                        self.recrypt(
                            value,
                            crypto::permission_item(old.record(), slot as u32),
                            crypto::permission_item(id, slot as u32),
                        );
                    }
                }
            }
            Kind::View
            | Kind::Procedure
            | Kind::Function
            | Kind::Package
            | Kind::Field
            | Kind::Publication => {}
            Kind::Other => return Err(Error::NotImplemented),
        }
        // The rights granted to `DB:Public`, in a slot of the object itself, are
        // made anew, with a random part of their own, for the object's new id;
        // with none left there is no slot.
        if let Some(slot) = dd::public_rights_slot(old.kind) {
            slot_mut(&mut slots, slot);
            if let dd::PublicRights::Rights(rights) = dict.public_rights(old, Some(&self.key))
                && rights != 0
                && keeps_public_rights(old.kind)
                && !self.plan.fails_on_public
            {
                let salt: [u8; 4] = login_salts().secret_first[..4].try_into().unwrap();
                let mut blob = crypto::public_rights_blob(rights, salt);
                let item = crypto::public_rights_item(id, slot as u32);
                crypto::crypt(self.scheme, &self.key, item, &mut blob);
                put(&mut slots, slot, blob.to_vec());
            }
        }
        slots.sort_by_key(|(slot, _)| *slot);
        let borrowed: Vec<(usize, &[u8])> = slots.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let stored = NewObject {
            id,
            parent,
            kind: stored_kind(old.kind),
            name: &name,
            info1: info1.unwrap_or(i32::MIN),
            info2,
            slots: &borrowed,
        };
        // The comment is allocated in the memo file before the rest of a long
        // property is.
        let raw =
            dd::create::comment_bytes(source, old.record()).map_err(|_| Error::CorruptTable)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let text = &raw[..end];
        // The comment is stored as it was set: a group's with its NUL
        // (the original sets it after making the group), the database's
        // without, the others' as they were.
        let nul = match old.kind {
            Kind::Database => false,
            Kind::Group => true,
            _ => raw.last() == Some(&0),
        };
        let mut commented = vec![0u8; dd::create::RECORD_LENGTH];
        match nul {
            true => dd::create::set_comment(&mut commented, memo_path, text),
            false => dd::create::set_comment_strlen(&mut commented, memo_path, text),
        }
        .map_err(|_| Error::CorruptTable)?;
        // A trigger is made with its properties set one at a time.
        let mut record = match old.kind {
            Kind::Trigger => dd::create::object_record_stepwise(
                &stored,
                memo_path,
                &[
                    dd::trigger::EVENT_TYPE,
                    dd::trigger::TRIGGER_TYPE,
                    dd::trigger::CONTAINER_TYPE,
                    dd::trigger::CONTAINER,
                    dd::trigger::FUNCTION_NAME,
                    dd::trigger::PRIORITY,
                    dd::trigger::OPTIONS,
                ],
            ),
            _ => dd::create::object_record_continued(&stored, memo_path),
        }
        .map_err(|_| Error::CorruptTable)?;
        let comment = dd::create::COMMENT..dd::create::COMMENT + 8;
        record[comment.clone()].copy_from_slice(&commented[comment]);
        Ok(record)
    }
}

/// The object type a record stores for `kind`.
fn stored_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Database => "Database",
        Kind::Table => "Table",
        Kind::Field => "Field",
        Kind::IndexFile => "Index",
        Kind::Index => "Key",
        Kind::View => "View",
        Kind::Procedure => "StoredProc",
        Kind::Function => "Function",
        Kind::Package => "Package",
        Kind::Trigger => "Trigger",
        Kind::User => "User",
        Kind::Group => "Group",
        Kind::Permission => "Permission",
        Kind::Relation => "Relation",
        Kind::Link => "LinkedDD",
        Kind::Publication => "Publicatn",
        Kind::Article => "Article",
        Kind::Subscription => "Subscriptn",
        Kind::Other => "",
    }
}
