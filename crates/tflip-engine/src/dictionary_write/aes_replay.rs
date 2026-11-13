//! Writes to AES dictionaries, replayed from what the original wrote
//! (`fixtures/aes/dictwrite`, made by `tools/ace/crypto/aeswrite.sh`): its
//! scripts run on copies of the dictionaries it created, and on ones tflip
//! creates with its first script, and after each the files are compared with
//! the original's, deciphered.

use std::path::{Path, PathBuf};

use crate::{Session, connection, statement};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/dictwrite")
}

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/ace/crypto/aeswrite")
}

/// Runs the probe script `script` on the dictionaries in `dir`, returning
/// the code each failed step ended in, by its number (the probe's: 1 for the
/// first line that is no comment).
pub(super) fn run(session: &Session, dir: &Path, script: &str) -> Vec<(usize, u32)> {
    let folder = dir.to_string_lossy();
    let (mut connection, mut failed) = (None, Vec::new());
    let lines = script
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    for (step, line) in lines.enumerate() {
        let line = line.replace("%D", &folder).replace('\\', "/");
        let step = step + 1;
        let mut done = |error: crate::Error| {
            if std::env::var("AES_DEBUG").is_ok() {
                eprintln!("step {step}: {line}: {error:?}");
            }
            failed.push((step, error.to_ace().code))
        };
        if let Some(cs) = line.strip_prefix("connect ") {
            connection = connection::connect(session, cs).map_err(&mut done).ok();
        } else if let Some(cs) = line.strip_prefix("ddcreate101 ") {
            connection = super::create_dictionary_connect_api(session, cs)
                .map_err(&mut done)
                .ok();
        } else if let Some(rest) = line.strip_prefix("ddcreate ") {
            let (flag, path) = rest.split_once(' ').unwrap();
            connection = super::create_dictionary_api(session, path, flag.parse().unwrap())
                .map_err(&mut done)
                .ok();
        } else if let Some(sql) = line.strip_prefix("sql ") {
            let handle = statement::create(session, connection.unwrap()).unwrap();
            statement::execute_direct(session, handle, sql, true).map_or_else(done, drop);
            statement::close(session, handle).ok();
        } else if line == "disconnect"
            && let Some(connection) = connection.take()
        {
            connection::disconnect(session, connection).unwrap();
        }
    }
    failed
}

/// The failed steps and their codes the original's result `json` of a script
/// records.
fn recorded_failures(json: &str) -> Vec<(usize, u32)> {
    let json: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut failed: Vec<_> = json
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(key, value)| {
            let step = key.strip_suffix("_rc")?.parse().ok()?;
            Some((step, value.as_u64()? as u32)).filter(|(_, code)| *code != 0)
        })
        .collect();
    failed.sort();
    failed
}

/// Where a record's properties start in a dictionary of the default layout;
/// an AES one's records have their IV before the fields.
const PROPERTY: usize = 223;

/// Whether the dictionary `add` has the AES layout.
pub(super) fn is_aes(add: &[u8]) -> bool {
    u32::from_le_bytes(add[0x170..0x174].try_into().unwrap()) != 0
}

/// Blanks the random material of the AES file `bytes` (dictionary or
/// table): its verifier, nonce, IV counter, salt and descriptor IV.
pub(super) fn blank_material(bytes: &mut [u8]) {
    bytes[0x38..0x4C].fill(0);
    if is_aes(bytes) {
        bytes[0x194..0x1C8].fill(0);
    }
}

/// A dictionary's `.add` as compared: its records deciphered, its random
/// material and the secrets tflip salts afresh blanked (the Database login
/// slots 0, 1 and 4, and each user's password secret, slot 0); with
/// `own_key`, for a dictionary tflip made with a key of its own, also what
/// that key enciphers (users' memberships and create rights, permissions).
pub(super) fn normalized(add: &[u8], own_key: bool) -> Vec<u8> {
    let mut bytes = tflip_storage::dd::deciphered(add, b"ddpw");
    blank_material(&mut bytes);
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let (data, slots, record) = (word(&bytes, 0x20), word(&bytes, 0x18), word(&bytes, 0x24));
    let shift = record - 524;
    for slot in 0..slots {
        let base = data + slot * record;
        let masked: &[usize] = match &bytes[base + 13 + shift..base + 23 + shift] {
            kind if kind.starts_with(b"Database") => &[0, 1, 4],
            kind if kind.starts_with(b"User") && own_key => &[0, 1, 6],
            kind if kind.starts_with(b"User") => &[0],
            kind if kind.starts_with(b"Group") && own_key => &[6],
            kind if kind.starts_with(b"Permission") && own_key => &[1, 2, 3, 4],
            _ => continue,
        };
        let property = base + PROPERTY + shift;
        let (mut at, mut index) = (property, 0);
        while at + 2 <= property + 275 {
            let length = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            if length == 0xFFFF {
                at += 2;
                index += 1;
                continue;
            }
            let length = usize::from(length);
            if masked.contains(&index) {
                bytes[at + 2..(at + 2 + length).min(property + 275)].fill(0);
            }
            at += 2 + length;
            index += 1;
        }
    }
    bytes
}

/// The key and nonce of the AES file (dictionary or table) `bytes`, which
/// the dictionary password makes.
pub(super) fn key_of(bytes: &[u8]) -> (Vec<u8>, [u8; 4]) {
    let key_len = if bytes[0x190] == 5 { 16 } else { 32 };
    let derived = tflip_storage::crypto::aes_derive(b"ddpw", &bytes[0x1A0..0x1C0], key_len);
    (
        derived[..key_len].to_vec(),
        bytes[0x194..0x198].try_into().unwrap(),
    )
}

/// Whether the AES file `bytes` is encrypted.
pub(super) fn encrypted(bytes: &[u8]) -> bool {
    bytes[0x58] & 0x04 != 0
}

/// Whether the 24 bytes `record` are a slab's record: its marker, a count of
/// chunks and their size in blocks that fit the slab, and no chunk in use
/// past the last. The marker alone shows in the key stream deciphered from
/// unused space once in 256 keys.
fn is_slab_record(record: &[u8]) -> bool {
    let (chunks, size) = (usize::from(record[1]), usize::from(record[3]));
    record[0] == 0xFB
        && chunks > 0
        && size > 0
        && chunks * size <= 124
        && (chunks..128).all(|chunk| record[8 + chunk / 8] >> (chunk % 8) & 1 == 0)
}

/// A dictionary's memo file `am`, of the dictionary `add`, as compared: in
/// one that encrypts its records, each value a record refers to and each
/// slab's record deciphered, their IVs zeroed, and what is left of the
/// blocks after a value and the chunks a slab has free blanked: the original
/// enciphers a value to the end of its blocks, the bytes after it what its
/// buffer held.
pub(super) fn normalized_memo(am: &[u8], add: &[u8]) -> Vec<u8> {
    let mut bytes = am.to_vec();
    if !encrypted(add) || !is_aes(add) {
        return bytes;
    }
    let (key, nonce) = key_of(add);
    let block = u32::from_be_bytes(am[4..8].try_into().unwrap()) as usize;
    let mut done = std::collections::HashSet::new();
    let mut decipher = |bytes: &mut Vec<u8>, at: usize, length: usize| {
        if at + 8 + length <= bytes.len() && done.insert(at) {
            let iv: [u8; 8] = bytes[at..at + 8].try_into().unwrap();
            // Nothing was enciphered behind a zero IV or in zeros:
            // deciphering unused space would show the key stream, whose
            // first byte is a slab's marker once in 256 keys.
            if iv != [0; 8] && bytes[at + 8..at + 8 + length].iter().any(|&b| b != 0) {
                tflip_storage::crypto::aes_ctr_xor(
                    &key,
                    &iv,
                    &nonce,
                    &mut bytes[at + 8..at + 8 + length],
                );
            }
            bytes[at..at + 8].fill(0);
        }
    };
    let add = tflip_storage::dd::deciphered(add, b"ddpw");
    let word = |at: usize| u32::from_le_bytes(add[at..at + 4].try_into().unwrap()) as usize;
    let (data, slots) = (word(0x20), word(0x18));
    for slot in 0..slots {
        let base = data + slot * word(0x24);
        // More Property, then Comment: a block and a length each.
        for at in [base + 506, base + 523] {
            if word(at) != 0 {
                let (start, length) = (word(at) * block, word(at + 4));
                let room = (length + 8).div_ceil(block) * block - 8;
                decipher(&mut bytes, start, room);
                bytes[start + 8 + length..start + 8 + room].fill(0);
            }
        }
    }
    // The slabs start at multiples of 128 blocks, their chunks after their
    // records' 4 blocks.
    // Blocks of 8 bytes put the first multiple inside the header (its free
    // lists), which is no slab whatever a deciphering of it happens to show.
    let mut slab = 128 * block;
    while slab < 0x900 {
        slab += 128 * block;
    }
    while slab + 32 <= bytes.len() {
        // What is no slab (the header's end, a run) stays as it is.
        let stored = bytes[slab..slab + 32].to_vec();
        decipher(&mut bytes, slab, 24);
        if !is_slab_record(&bytes[slab + 8..slab + 32]) {
            bytes[slab..slab + 32].copy_from_slice(&stored);
        } else {
            let (chunks, size) = (bytes[slab + 9] as usize, bytes[slab + 11] as usize);
            for chunk in 0..chunks {
                let in_use = bytes[slab + 16 + chunk / 8] >> (chunk % 8) & 1 != 0;
                let at = slab + (4 + chunk * size) * block;
                if !in_use && at + size * block <= bytes.len() {
                    bytes[at..at + size * block].fill(0);
                }
            }
        }
        slab += 128 * block;
    }
    bytes
}

/// A table bound to an AES dictionary as compared: one laid out for AES has
/// its random nonce, IV counter and descriptor IV blanked, and if armed its
/// descriptors and records deciphered (the records of one that was decrypted
/// are in the clear, their IVs zeroed).
fn normalized_table(table: &[u8]) -> Vec<u8> {
    if table.len() < 0x1C8 || u32::from_le_bytes(table[0x170..0x174].try_into().unwrap()) == 0 {
        return table.to_vec();
    }
    let mut bytes = tflip_storage::dd::deciphered(table, b"ddpw");
    if encrypted(table) {
        let (key, nonce) = key_of(table);
        let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
        let iv: [u8; 8] = bytes[0x1C0..0x1C8].try_into().unwrap();
        tflip_storage::crypto::aes_ctr_xor(&key, &iv, &nonce, &mut bytes[1000..data]);
    }
    blank_material(&mut bytes);
    bytes
}

/// The index `index` of the table `adt` as compared: in one that is
/// encrypted, its tree pages deciphered, their IVs zeroed, and the
/// dictionary's verifier blanked.
fn normalized_table_index(index: &[u8], adt: &[u8], scratch: &Path) -> Vec<u8> {
    // A restructure leaves the index in the clear.
    if !encrypted(adt) || index[0x19] & 0x02 == 0 {
        return index.to_vec();
    }
    let (key, nonce) = key_of(adt);
    let path = scratch.join("compare.adi");
    std::fs::write(&path, index).unwrap();
    let mut bytes = tflip_storage::adi::deciphered(&path, &key, &nonce).unwrap();
    bytes[0x20..0x34].fill(0);
    bytes
}

/// The memo file `adm` of the table `adt` (the one with the memo field `note`
/// at the 5th byte of its data) as compared: the header, the slabs' records
/// and the free runs' headers as they are, deciphered where they are
/// enciphered, and the values the records refer to deciphered and cut to
/// their length; the rest, which is what freed values left, blanked.
fn normalized_table_memo(adm: &[u8], adt: &[u8]) -> Vec<u8> {
    let block = u16::from_be_bytes(adm[6..8].try_into().unwrap()) as usize;
    let key = encrypted(adt).then(|| key_of(adt));
    let at = |block_number: usize| block_number * block;
    let word = |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let decipher = |start: usize, length: usize| -> Vec<u8> {
        let stored = adm[start..start + 8 + length].to_vec();
        let iv: [u8; 8] = stored[..8].try_into().unwrap();
        let mut bytes = stored[8..].to_vec();
        if let Some((key, nonce)) = &key
            && iv != [0; 8]
            && bytes.iter().any(|&b| b != 0)
        {
            tflip_storage::crypto::aes_ctr_xor(key, &iv, nonce, &mut bytes);
        }
        bytes
    };
    let mut out = vec![0; adm.len()];
    out[..0x1000].copy_from_slice(&adm[..0x1000]);
    // The slabs.
    let mut slab = 128;
    while at(slab) + 32 <= adm.len() {
        let record = decipher(at(slab), 24);
        if is_slab_record(&record) {
            out[at(slab) + 8..at(slab) + 32].copy_from_slice(&record);
        }
        slab += 128;
    }
    // The free runs: their sizes and links, behind an IV, which the rest of
    // the block never tells.
    for class in 0..128 {
        let mut run = word(adm, 0x404 + 8 * class) as usize;
        while run != 0 && at(run) + 16 <= adm.len() {
            let header = decipher(at(run), 8);
            out[at(run) + 8..at(run) + 16].copy_from_slice(&header);
            run = word(&header, 4) as usize;
        }
    }
    // The values.
    let records = tflip_storage::dd::deciphered(adt, b"ddpw");
    let (data, length, slots) = (
        word(&records, 0x20),
        word(&records, 0x24),
        word(&records, 0x18),
    );
    // The memo fields' places, from the descriptors after the header and the
    // AES extension.
    let start = 400 + word(&records, 0x170) as usize;
    let mut descriptors = records[start..data as usize].to_vec();
    if let Some((key, nonce)) = &key {
        let iv: [u8; 8] = records[0x1C0..0x1C8].try_into().unwrap();
        tflip_storage::crypto::aes_ctr_xor(key, &iv, nonce, &mut descriptors);
    }
    let memos: Vec<usize> = descriptors
        .chunks_exact(tflip_storage::adt::DESCRIPTOR_SIZE)
        .map(|bytes| tflip_storage::adt::parse_field(bytes.try_into().unwrap()))
        .filter(|field| field.is_memo())
        .map(|field| usize::from(field.offset))
        .collect();
    for slot in 0..slots as usize {
        let record = &records[data as usize + slot * length as usize..][..length as usize];
        for &place in &memos {
            let (value, stored) = (
                word(record, place) as usize,
                word(record, place + 4) as usize,
            );
            if !tflip_storage::adt::is_active_clear(record[0]) || value == 0 || stored == 0 {
                continue;
            }
            let bytes = decipher(at(value), stored);
            out[at(value) + 8..at(value) + 8 + stored].copy_from_slice(&bytes);
        }
    }
    out
}

/// Where `ours` and `theirs` first differ, described.
pub(super) fn first_difference(ours: &[u8], theirs: &[u8]) -> Option<String> {
    if ours == theirs {
        return None;
    }
    let at = ours
        .iter()
        .zip(theirs)
        .position(|(a, b)| a != b)
        .unwrap_or(ours.len().min(theirs.len()));
    let from = at.saturating_sub(8);
    Some(format!(
        "lengths {} and {}, first at {at:#x}: ours {:02x?} theirs {:02x?}",
        ours.len(),
        theirs.len(),
        &ours[from..(at + 16).min(ours.len())],
        &theirs[from..(at + 16).min(theirs.len())],
    ))
}

/// The probe's scripts replayed on the dictionaries it created.
#[test]
fn writes_aes_dictionaries_as_the_original() {
    replay(false);
}

/// The probe's scripts replayed on dictionaries tflip creates with its
/// first, `00_create`.
#[test]
fn creates_aes_dictionaries_as_the_original() {
    replay(true);
}

/// The dictionaries the probe's scripts work on, and those the last makes.
const DICTIONARIES: [&str; 20] = [
    "c", "e", "f", "h", "i", "j", "k", "m", "n", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y",
    "z",
];

/// Runs the probe's scripts, from the first if `create`, else on copies of
/// the dictionaries the original created, comparing what they fail with and
/// the files after each with the original's.
fn replay(create: bool) {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-aes-replay-{create}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let snap = fixtures().join("snap");
    for name in DICTIONARIES {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        let from = snap.join("00_create").join(name);
        if create || !from.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&from).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join(name).join(entry.file_name())).unwrap();
        }
    }
    let session = Session::new();
    let mut failures = Vec::new();
    let mut steps: Vec<_> = std::fs::read_dir(tools())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| create || !p.file_name().unwrap().to_string_lossy().starts_with("00"))
        .collect();
    steps.sort();
    // AES_ONLY=10 replays the script 10_… alone.
    let only = std::env::var("AES_ONLY").unwrap_or_default();
    let mut previous = "00_create".to_owned();
    for step in steps {
        let stem = step.file_stem().unwrap().to_string_lossy().into_owned();
        if !stem.starts_with(&only) {
            previous.clone_from(&stem);
            continue;
        }
        // Each script starts from what the original's last left, so a
        // difference in one does not carry into the next.
        // Scripts of 40_ on start from nothing, in a folder of their own: the
        // process keeps a dictionary it opened by its path.
        let from_nothing = stem.as_str() >= "40";
        let work = match from_nothing {
            true => dir.join(&stem),
            false => dir.clone(),
        };
        if from_nothing {
            for name in DICTIONARIES {
                std::fs::create_dir_all(work.join(name)).unwrap();
            }
        } else if !create {
            for name in DICTIONARIES {
                let (to, from) = (dir.join(name), snap.join(&previous).join(name));
                let _ = std::fs::remove_dir_all(&to);
                std::fs::create_dir_all(&to).unwrap();
                for entry in std::fs::read_dir(from).into_iter().flatten() {
                    let entry = entry.unwrap();
                    std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
                }
            }
        }
        previous.clone_from(&stem);
        let failed = run(&session, &work, &std::fs::read_to_string(&step).unwrap());
        let recorded = recorded_failures(
            &std::fs::read_to_string(fixtures().join(format!("{stem}.json"))).unwrap(),
        );
        if failed != recorded {
            failures.push(format!("{stem}: fails {failed:?}, theirs {recorded:?}"));
        }
        for name in DICTIONARIES {
            let (ours, theirs) = (work.join(name), snap.join(&stem).join(name));
            // A script that starts from nothing (40_ and on) keeps only its
            // own dictionaries in its snapshot.
            if std::fs::read_dir(&theirs).is_ok_and(|mut files| files.next().is_none())
                || !theirs.exists()
            {
                continue;
            }
            let mut files: Vec<_> = [&ours, &theirs]
                .iter()
                .filter_map(|d| std::fs::read_dir(d).ok())
                .flatten()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            files.sort();
            files.dedup();
            let read = |root: &Path, file: &str| std::fs::read(root.join(file)).ok();
            let add = format!("{name}.add");
            // The dictionaries tflip made with a key of its own: all of
            // them if it made them all, else those of the last script.
            let own_key = create
                || matches!(
                    name,
                    "h" | "i"
                        | "m"
                        | "n"
                        | "q"
                        | "r"
                        | "s"
                        | "t"
                        | "u"
                        | "v"
                        | "w"
                        | "x"
                        | "y"
                        | "z"
                );
            let (our_add, their_add) = (read(&ours, &add), read(&theirs, &add));
            let keyed = our_add.as_ref().zip(their_add.as_ref());
            for file in files {
                let (a, b) = (read(&ours, &file), read(&theirs, &file));
                let (Some(a), Some(b)) = (a.clone(), b.clone()) else {
                    failures.push(format!(
                        "{stem} {name}/{file}: ours {}, theirs {}",
                        a.is_some(),
                        b.is_some()
                    ));
                    continue;
                };
                // The files an ALTER TABLE put aside are compared as the
                // tables they were, the table's own BAK.
                let (base, aside) = match file.strip_suffix(".BAK") {
                    Some(base) => (base, ".BAK"),
                    None => (file.as_str(), ""),
                };
                let (a, b) = match (base.rsplit('.').next().unwrap(), keyed) {
                    ("add", _) => (normalized(&a, own_key), normalized(&b, own_key)),
                    ("am", Some((x, y))) if file.starts_with(name) => {
                        (normalized_memo(&a, x), normalized_memo(&b, y))
                    }
                    // An encrypted dictionary's index keeps its verifier.
                    ("ai", Some((x, y))) if encrypted(x) && is_aes(x) => {
                        let index = |bytes: &[u8], add: &[u8]| {
                            let (key, nonce) = key_of(add);
                            let path = dir.join(format!("compare-{name}.ai"));
                            std::fs::write(&path, bytes).unwrap();
                            let mut bytes =
                                tflip_storage::adi::deciphered(&path, &key, &nonce).unwrap();
                            bytes[0x20..0x34].fill(0);
                            bytes
                        };
                        (index(&a, x), index(&b, y))
                    }
                    ("adt", _) => (normalized_table(&a), normalized_table(&b)),
                    (kind @ ("adi" | "adm"), _) => {
                        let table = format!("{}.adt{aside}", base.rsplit_once('.').unwrap().0);
                        let (x, y) = (read(&ours, &table), read(&theirs, &table));
                        let (Some(x), Some(y)) = (x, y) else {
                            failures.push(format!("{stem} {name}/{file}: no table"));
                            continue;
                        };
                        match kind {
                            "adi" => (
                                normalized_table_index(&a, &x, &dir),
                                normalized_table_index(&b, &y, &dir),
                            ),
                            // A table of an RC4 dictionary is laid out as any.
                            _ if !is_aes(&x) || !is_aes(&y) => (a, b),
                            _ => (normalized_table_memo(&a, &x), normalized_table_memo(&b, &y)),
                        }
                    }
                    _ => (a, b),
                };
                if let Some(difference) = first_difference(&a, &b) {
                    failures.push(format!("{stem} {name}/{file}: {difference}"));
                }
            }
        }
    }
    if std::env::var("AES_KEEP").is_ok() {
        eprintln!("KEPT {}", dir.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
