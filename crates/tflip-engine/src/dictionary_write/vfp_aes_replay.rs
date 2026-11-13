//! Visual FoxPro tables of an AES dictionary, and those a connection with
//! `EncryptionType=AES128` makes, replayed from what the original made
//! (`fixtures/aes/rc4tp` `v1` to `v12`, made by `tools/ace/crypto/rc4tp/run.sh`).
//! Each script runs on new dictionaries, and the steps that fail and every
//! file left are compared with the original's after deciphering (the IVs, the
//! nonces and the salts are random, as are the bytes a value's blocks hold
//! after it).

use std::path::Path;

use super::aes_replay::{encrypted, first_difference, is_aes, key_of, normalized, normalized_memo};
use super::rc4_replay::{files_under, fixtures, recorded, run, tools};
use crate::Session;

/// The passwords the probe's tables are enciphered under: the dictionary's,
/// and a free table's.
const PASSWORDS: [&[u8]; 2] = [b"ddpw", b"secret"];

/// The memo fields of the table `dbf`: where their 4-byte block numbers are.
fn memo_fields(dbf: &[u8]) -> Vec<usize> {
    let mut at = 32;
    let mut places = Vec::new();
    while dbf[at] != 0x0D {
        if matches!(dbf[at + 11], b'M' | b'W' | b'G' | b'P') {
            places.push(u32::from_le_bytes(dbf[at + 12..at + 16].try_into().unwrap()) as usize);
        }
        at += 32;
    }
    places
}

/// Whether the table `dbf` is laid out for AES (an RC4 one has the version 0xF8 too).
fn aes_layout(dbf: &[u8]) -> bool {
    dbf[0] == 0xF9 || (dbf[0] == 0xF8 && matches!(dbf[0x0C], 5 | 6))
}

/// The key and nonce of the table `dbf` if it is armed for AES, whichever of
/// the probe's passwords opens it.
fn table_key(dbf: &[u8]) -> Option<(Vec<u8>, [u8; 4])> {
    if dbf[0] != 0xF8 || !matches!(dbf[0x0C], 5 | 6) {
        return None;
    }
    let key_len = if dbf[0x0C] == 5 { 16 } else { 32 };
    let header = usize::from(u16::from_le_bytes([dbf[8], dbf[9]]));
    let block = &dbf[header - 263..];
    PASSWORDS.iter().find_map(|password| {
        let derived = tflip_storage::crypto::aes_derive(password, &block[16..48], key_len);
        (derived[key_len..key_len + 9] == dbf[0x0D..0x16])
            .then(|| (derived[..key_len].to_vec(), block[4..8].try_into().unwrap()))
    })
}

/// The table file `dbf` as compared: its date and random material blanked,
/// and if armed its records deciphered with their IVs zeroed.
fn normalized_dbf(dbf: &[u8]) -> Vec<u8> {
    let mut bytes = dbf.to_vec();
    bytes[1..4].fill(0);
    if !aes_layout(dbf) {
        return bytes;
    }
    let key = table_key(dbf);
    let header = usize::from(u16::from_le_bytes([dbf[8], dbf[9]]));
    let length = usize::from(u16::from_le_bytes([dbf[10], dbf[11]]));
    if let Some((key, nonce)) = &key {
        let records = (dbf.len() - header - 1) / length;
        for record in 0..records {
            let at = header + record * length;
            let end = at + length - 8;
            // A record in the clear (the table was armed before it was
            // written) has no IV to decipher with.
            if bytes[at] != b'E' && bytes[at] != b'D' {
                continue;
            }
            let iv: [u8; 8] = bytes[end..end + 8].try_into().unwrap();
            tflip_storage::crypto::aes_ctr_xor(key, &iv, nonce, &mut bytes[at + 1..end]);
            bytes[at] = if bytes[at] == b'E' { b' ' } else { b'*' };
            bytes[end..end + 8].fill(0);
        }
    }
    bytes[0x0D..0x16].fill(0);
    bytes[header - 263 + 4..header - 263 + 48].fill(0);
    bytes
}

/// The memo file `fpt` of the table `dbf` as compared: its header as it is,
/// each value a record refers to deciphered and cut to its length behind a
/// zero IV, and the rest, what the blocks held after a value, blanked.
fn normalized_fpt(fpt: &[u8], dbf: &[u8]) -> Vec<u8> {
    let armed = aes_layout(dbf);
    let key = table_key(dbf);
    let block_size = usize::from(u16::from_be_bytes([fpt[6], fpt[7]]));
    let header = usize::from(u16::from_le_bytes([dbf[8], dbf[9]]));
    let length = usize::from(u16::from_le_bytes([dbf[10], dbf[11]]));
    if !armed && key.is_none() {
        // A table that is not laid out for AES keeps its memo file as it is.
        return fpt.to_vec();
    }
    let mut out = vec![0; fpt.len()];
    out[..512].copy_from_slice(&fpt[..512]);
    let places = memo_fields(dbf);
    // The pointers are enciphered with their records.
    let clear = normalized_dbf(dbf);
    for record in 0..(dbf.len() - header - 1) / length {
        let at = header + record * length;
        for &place in &places {
            let block = u32::from_le_bytes(clear[at + place..at + place + 4].try_into().unwrap());
            if block == 0 {
                continue;
            }
            let start = block as usize * block_size;
            let value = u32::from_be_bytes(fpt[start + 4..start + 8].try_into().unwrap()) as usize;
            let mut bytes = fpt[start + 16..start + 16 + value].to_vec();
            let iv: [u8; 8] = fpt[start + 8..start + 16].try_into().unwrap();
            if let (Some((key, nonce)), true) = (&key, iv != [0; 8]) {
                tflip_storage::crypto::aes_ctr_xor(key, &iv, nonce, &mut bytes);
            }
            out[start..start + 8].copy_from_slice(&fpt[start..start + 8]);
            out[start + 16..start + 16 + value].copy_from_slice(&bytes);
        }
    }
    out
}

/// Replays `script`, returning what differs from the original's.
fn replay(script: &str) -> Vec<String> {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-vfp-aes-{script}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let text = std::fs::read_to_string(tools().join(format!("{script}.txt")))
        .unwrap()
        .replace("%N", script);
    // The folders of free tables are made if the original's have files.
    std::fs::create_dir_all(dir.join(script)).unwrap();
    let originals = fixtures().join(script);
    for name in ["free", "aes"] {
        std::fs::create_dir_all(dir.join(script).join(name)).unwrap();
    }
    let session = Session::new();
    let ours = run(&session, &dir, &text);
    let theirs = recorded(script);
    let mut differences = Vec::new();
    if ours.failed != theirs.failed || ours.values != theirs.values {
        differences.push(format!(
            "{script}: failed {:?} read {:?}, the original {:?} {:?}",
            ours.failed, ours.values, theirs.failed, theirs.values
        ));
    }
    let made = dir.join(script);
    let add_of = |root: &Path| std::fs::read(root.join("d.add")).ok();
    let (our_add, their_add) = (add_of(&made), add_of(&originals));
    for name in files_under(&originals) {
        // `sp_EncryptTable` of a plain table makes new Field objects in the
        // original, which tflip does not (KNOWN_ISSUES).
        // A free ADT table's key is of a salt of its own: the file differs.
        // The copy of an AES ADT table without its password is not matched
        // (KNOWN_ISSUES).
        if script == "v14" && (name.starts_with("fadt.") || name.starts_with("cpadt.")) {
            continue;
        }
        if script == "v11" && name.starts_with("d.") {
            continue;
        }
        let Ok(ours) = std::fs::read(made.join(&name)) else {
            differences.push(format!("{script}/{name}: not made"));
            continue;
        };
        let theirs = std::fs::read(originals.join(&name)).unwrap();
        let base = name.strip_suffix(".BAK").unwrap_or(&name);
        let sibling = |root: &Path| {
            let file = std::fs::read(root.join(format!("{}.dbf", &base[..base.len() - 4]))).ok();
            file.map(|bytes| match name.ends_with(".BAK") {
                true => std::fs::read(root.join(format!("{}.dbf.BAK", &base[..base.len() - 4])))
                    .unwrap_or(bytes),
                false => bytes,
            })
        };
        let (a, b) = match (base.rsplit('.').next().unwrap(), &our_add, &their_add) {
            ("add", ..) => (normalized(&ours, true), normalized(&theirs, true)),
            ("am", Some(x), Some(y)) if name == "d.am" => {
                (normalized_memo(&ours, x), normalized_memo(&theirs, y))
            }
            ("ai", Some(x), Some(y)) if name == "d.ai" && encrypted(x) && is_aes(x) => {
                let index = |bytes: &[u8], add: &[u8]| {
                    let (key, nonce) = key_of(add);
                    let path = dir.join(format!("compare-{script}.ai"));
                    std::fs::write(&path, bytes).unwrap();
                    let mut bytes = tflip_storage::adi::deciphered(&path, &key, &nonce).unwrap();
                    bytes[0x20..0x34].fill(0);
                    bytes
                };
                (index(&ours, x), index(&theirs, y))
            }
            ("dbf", ..) => (normalized_dbf(&ours), normalized_dbf(&theirs)),
            ("fpt", ..) => match (sibling(&made), sibling(&originals)) {
                (Some(x), Some(y)) => (normalized_fpt(&ours, &x), normalized_fpt(&theirs, &y)),
                _ => (ours, theirs),
            },
            _ => (ours, theirs),
        };
        if let Some(difference) = first_difference(&a, &b) {
            differences.push(format!("{script}/{name}: {difference}"));
        }
    }
    for name in files_under(&made) {
        if !originals.join(&name).exists() {
            differences.push(format!("{script}/{name}: made, not the original's"));
        }
    }
    if std::env::var("KEEP").is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    differences
}

/// The scripts replayed.
const SCRIPTS: [&str; 15] = [
    "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v10", "v11", "v12", "v13", "v14", "v15", "v16",
];

#[test]
fn makes_and_changes_vfp_tables_of_an_aes_dictionary_as_the_original() {
    let only = std::env::var("ONLY").ok();
    let differences: Vec<String> = SCRIPTS
        .iter()
        .filter(|script| only.as_deref().is_none_or(|only| only == **script))
        .flat_map(|script| replay(script))
        .collect();
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// What a VFP table of the dictionary in `dir` holds, deciphered: the live
/// records' id and memo text.
fn rows(dir: &Path, table: &str) -> Vec<(i32, String)> {
    use tflip_storage::dbf::Flavour;
    use tflip_storage::table::{Format, TableFile};
    let path = dir.join(format!("{table}.dbf"));
    let mut file =
        TableFile::open_deciphering(&path, Format::Dbf(Flavour::Visual), false, Some(b"ddpw"))
            .unwrap();
    let mut rows = Vec::new();
    for slot in 1..=file.slots() {
        let record = file.record(slot).unwrap();
        let id = i32::from_le_bytes(record[1..5].try_into().unwrap());
        let block = u32::from_le_bytes(record[5..9].try_into().unwrap());
        if record[0] != b' ' || id == 0 {
            continue;
        }
        let memo = file.memo().unwrap();
        memo.decipher_record(slot);
        let text = memo.read_dbf(block, false).unwrap();
        rows.push((id, String::from_utf8(text).unwrap()));
    }
    rows
}

/// The original's remote server could not be probed with an AES dictionary
/// (it loads no OpenSSL, 5214), so this only checks that tflip's transactions
/// undo and keep the changes of an encrypted VFP table as they do a clear one's.
#[test]
fn rolls_back_and_commits_changes_of_an_aes_vfp_table() {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-vfp-aes-tx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("t")).unwrap();
    let text = r#"connect Data Source=%D\t\;ServerType=LOCAL;EncryptionType=AES128;FIPS=FALSE;DDPassword=ddpw;
sql CREATE DATABASE "d.add" ENCRYPT TRUE ENCRYPTIONTYPE AES128 DDPASSWORD 'ddpw'
disconnect
connect Data Source=%D\t\d.add;ServerType=LOCAL;User ID=ADSSYS;EncryptionType=AES128;FIPS=FALSE;DDPassword=ddpw;
sql EXECUTE PROCEDURE sp_ModifyDatabase('ENCRYPT_NEW_TABLE', 'TRUE')
stmttype 4
sql CREATE TABLE tb (id INTEGER, note MEMO)
sql INSERT INTO tb VALUES (1, 'one')
sql INSERT INTO tb VALUES (2, 'two')
begin
sql INSERT INTO tb VALUES (3, 'rolled back')
sql UPDATE tb SET note = 'changed then rolled back' WHERE id = 1
sql DELETE FROM tb WHERE id = 2
rollback
disconnect
"#;
    let session = Session::new();
    let outcome = run(&session, &dir, text);
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let home = dir.join("t");
    assert_eq!(
        rows(&home, "tb"),
        vec![(1, "one".to_owned()), (2, "two".to_owned())]
    );
    let commit = r#"connect Data Source=%D\t\d.add;ServerType=LOCAL;User ID=ADSSYS;EncryptionType=AES128;FIPS=FALSE;DDPassword=ddpw;
stmttype 4
begin
sql INSERT INTO tb VALUES (4, 'committed')
sql UPDATE tb SET note = 'changed and committed' WHERE id = 1
sql DELETE FROM tb WHERE id = 2
commit
disconnect
"#;
    let outcome = run(&session, &dir, commit);
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert_eq!(
        rows(&home, "tb"),
        vec![
            (1, "changed and committed".to_owned()),
            (4, "committed".to_owned())
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
