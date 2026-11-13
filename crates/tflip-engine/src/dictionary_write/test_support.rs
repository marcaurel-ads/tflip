//! Helpers the tests of the dictionary-write submodules share.

use super::*;

/// Asserts the connection's dictionary has a view named `name`.
pub(super) fn conn_has_view(session: &Session, connection: Handle, name: &str) {
    let conn = connection::get(session, connection).unwrap();
    let found = conn
        .with_database_opt(|db| {
            db.dictionary()
                .named(tflip_storage::dd::Kind::View, name)
                .is_some()
        })
        .unwrap_or(false);
    assert!(found, "view {name} visible on the connection");
}

pub(super) fn ops_fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/ops")
}

/// Zeroes the Database object's property region: the engine re-salts its
/// login secret on every write, and tflip leaves it, so masking lets the
/// rest of the file compare equal.
pub(super) fn mask_login(mut bytes: Vec<u8>) -> Vec<u8> {
    let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
    for b in &mut bytes[data + 223..data + 507] {
        *b = 0;
    }
    bytes
}

/// Zeroes only the Database login secret — its property slots 0, 1 and 4,
/// which the engine re-salts on every write — so the database's other
/// property slots can be compared byte for byte.
pub(super) fn mask_db_secret(mut bytes: Vec<u8>) -> Vec<u8> {
    let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
    let blob = &mut bytes[data + 223..data + 223 + 275];
    let (mut at, mut idx) = (0usize, 0usize);
    while at + 2 <= blob.len() {
        let len = u16::from_le_bytes([blob[at], blob[at + 1]]);
        if len == 0xffff {
            at += 2;
            idx += 1;
            continue;
        }
        let len = len as usize;
        if at + 2 + len > blob.len() {
            break;
        }
        if matches!(idx, 0 | 1 | 4) {
            blob[at + 2..at + 2 + len].fill(0);
        }
        at += 2 + len;
        idx += 1;
    }
    bytes
}

/// Blanks the random login secrets a dictionary re-salts on every write so
/// the deterministic rest compares equal: the Database object's login secret
/// (record 0, its property slots), every User object's password secret and
/// group membership (slots 0 and 1), and every Permission object's rights
/// (slots 1 and 2) — all enciphered with the per-dictionary key. Records are
/// the fixed dictionary layout.
pub(super) fn mask_user_secrets(bytes: Vec<u8>) -> Vec<u8> {
    let mut bytes = mask_login(bytes);
    let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
    let count = u32::from_le_bytes(bytes[0x18..0x1c].try_into().unwrap()) as usize;
    const RL: usize = 524;
    for rec in 1..count {
        let base = data + rec * RL;
        // Object Type is a 10-char field at record offset 13.
        let kind = &bytes[base + 13..base + 23];
        // Which property slots (0-based) carry enciphered, per-write-varying
        // data for this object kind.
        // The rights granted to `DB:Public` are in a slot of the object itself.
        let masked: &[usize] = if kind.starts_with(b"User") {
            &[0, 1, 6, 9] // the login secret, the memberships, the create rights
        } else if kind.starts_with(b"Group") {
            &[2, 6] // the create rights
        } else if kind.starts_with(b"Permission") {
            &[1, 2, 3, 4]
        } else if kind.starts_with(b"LinkedDD") {
            &[1, 2] // the wrapped link password, keyed by the per-dictionary key
        } else if kind.starts_with(b"Subscriptn") {
            &[2, 7, 9] // the enciphered password and connection string
        } else if kind.starts_with(b"Table") {
            &[12]
        } else if kind.starts_with(b"View") {
            &[5]
        } else if kind.starts_with(b"StoredProc") {
            &[6]
        } else if kind.starts_with(b"Function") {
            &[0]
        } else if kind.starts_with(b"Package") {
            &[2]
        } else if kind.starts_with(b"Publicatn") {
            &[1]
        } else {
            continue;
        };
        // Walk the property blob (offset 223: a u16 length then its bytes per
        // slot) and blank the data of the masked slots.
        let (mut at, mut slot) = (base + 223, 0usize);
        while at + 2 <= base + 223 + 275 {
            let len = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            if len == 0xffff {
                at += 2;
                slot += 1;
                continue;
            }
            let len = len as usize;
            if masked.contains(&slot) {
                bytes[at + 2..at + 2 + len].fill(0);
            }
            at += 2 + len;
            slot += 1;
        }
    }
    bytes
}

/// Blanks the slots in which objects keep the rights granted to `DB:Public`,
/// enciphered with the dictionary key and a random word the engine draws on
/// every write; tests compare what they decipher to (see
/// [`public_rights_of`]).
pub(super) fn mask_public_rights(mut bytes: Vec<u8>) -> Vec<u8> {
    let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
    let count = u32::from_le_bytes(bytes[0x18..0x1c].try_into().unwrap()) as usize;
    const RL: usize = 524;
    for rec in 0..count {
        let base = data + rec * RL;
        let kind = &bytes[base + 13..base + 23];
        let masked = [
            (&b"Database"[..], 16),
            (b"Table", 12),
            (b"View", 5),
            (b"StoredProc", 6),
            (b"Function", 0),
            (b"Package", 2),
            (b"LinkedDD", 2),
            (b"Group", 2),
            (b"Publicatn", 1),
            (b"Subscriptn", 7),
            (b"User", 9),
        ]
        .iter()
        .find(|(name, _)| kind.starts_with(name))
        .map(|&(_, slot)| slot);
        let Some(masked) = masked else { continue };
        let (mut at, mut slot) = (base + 223, 0usize);
        while at + 2 <= base + 223 + 275 {
            let len = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            if len == 0xffff {
                at += 2;
                slot += 1;
                continue;
            }
            let len = len as usize;
            if slot == masked && len == 32 {
                bytes[at + 2..at + 2 + len].fill(0);
            }
            at += 2 + len;
            slot += 1;
        }
    }
    bytes
}

/// What each object of the dictionary at `add` holds for `DB:Public`, by its
/// name: the rights its slot deciphers to with the key of the empty
/// administrator password.
pub(super) fn public_rights_of(add: &Path) -> Vec<(String, tflip_storage::dd::PublicRights)> {
    let dict = tflip_storage::dd::Dictionary::open(add).unwrap();
    let key = dict.dictionary_key(b"");
    dict.objects()
        .iter()
        .filter_map(|o| match dict.public_rights(o, key.as_deref()) {
            tflip_storage::dd::PublicRights::Absent => None,
            rights => Some((o.name.clone(), rights)),
        })
        .collect()
}

/// Copies the fixture table `from` to `to` as a free table: the fixtures are
/// tables of the dictionary the original added them to, which it does not add
/// to another.
pub(super) fn copy_free(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap();
    tflip_storage::dd::create::free_table(to).unwrap();
}

/// Builds the base dictionary the op fixtures start from: dict + table `t`
/// (id, s) with an index file `t.adi` (one order BY_ID) + view `v`.
pub(super) fn build_ops_base(session: &Session, dir: &Path) -> Handle {
    let fx = ops_fixtures();
    let add = dir.join("db.add");
    let connect = create_dictionary_api(session, &add.to_string_lossy(), 0).unwrap();
    std::fs::copy(fx.join("t.adt"), dir.join("t.adt")).unwrap();
    std::fs::copy(fx.join("t.adi"), dir.join("t.adi")).unwrap();
    // The fixture is a table of the dictionary it came from, which the
    // original does not add to another.
    tflip_storage::dd::create::free_table(&dir.join("t.adt")).unwrap();
    add_table_api(
        session,
        connect,
        "t",
        "t.adt",
        TableType::Adt,
        1,
        "t.adi",
        "",
    )
    .unwrap();
    add_view_api(session, connect, "v", "SELECT * FROM t", "").unwrap();
    connect
}
