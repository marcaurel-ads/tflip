//! `sp_SetDDEncryptionType`'s conversion replayed from what the original did
//! (`fixtures/aes/convert`, made by `tools/ace/crypto/aesconvert.sh`): each
//! script of `tools/ace/crypto/aesconvert` (`c_`, a conversion; `m_`, what
//! `sp_ModifyDatabase` does to the encryption of a dictionary) runs on a copy
//! of a dictionary that `before` holds, and the files it leaves are compared
//! with the original's, deciphered.

use std::path::{Path, PathBuf};

use super::aes_replay::{encrypted, first_difference, is_aes, key_of, normalized, run};
use crate::Session;

fn fixtures() -> PathBuf {
    std::env::var("AES_CONVERT_DIR").map_or_else(
        |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/convert"),
        PathBuf::from,
    )
}

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/ace/crypto/aesconvert")
}

/// The dictionary a script converts: the letter after its `c_`.
fn source_of(script: &str) -> &str {
    &script[2..3]
}

/// The index `ai` of the dictionary `add`, as compared: in one that is
/// enciphered with AES, its tree pages deciphered and the verifier blanked.
fn normalized_index(ai: &[u8], add: &[u8], scratch: &Path) -> Vec<u8> {
    if !encrypted(add) || !is_aes(add) {
        return ai.to_vec();
    }
    let (key, nonce) = key_of(add);
    let path = scratch.join("compare.ai");
    std::fs::write(&path, ai).unwrap();
    let mut bytes = tflip_storage::adi::deciphered(&path, &key, &nonce).unwrap();
    bytes[0x20..0x34].fill(0);
    bytes
}

/// The memo file `am` of the AES dictionary `add` as compared, enciphered or
/// not: the original writes a value to the end of its blocks and leaves the
/// space of a slab that no chunk takes, and what follows a value, as its
/// buffer held it, so those bytes are blanked; what is enciphered (a value
/// and a slab's record, unless its IV is zero, which is what was stored in the
/// clear) is deciphered and its IV zeroed.
fn normalized_am(am: &[u8], add: &[u8]) -> Vec<u8> {
    if !is_aes(add) {
        return am.to_vec();
    }
    let mut bytes = am.to_vec();
    let cipher = encrypted(add).then(|| key_of(add));
    let block = u32::from_be_bytes(am[4..8].try_into().unwrap()) as usize;
    let mut done = std::collections::HashSet::new();
    let mut decipher = |bytes: &mut Vec<u8>, at: usize, length: usize| {
        if at + 8 + length <= bytes.len() && done.insert(at) {
            let iv: [u8; 8] = bytes[at..at + 8].try_into().unwrap();
            if let (Some((key, nonce)), true) = (&cipher, iv != [0; 8]) {
                tflip_storage::crypto::aes_ctr_xor(
                    key,
                    &iv,
                    nonce,
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
        // A deleted record's values are not looked after.
        if add[base] != 0x04 {
            continue;
        }
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
    let mut slab = 128 * block;
    while slab + 32 <= bytes.len() {
        let stored = bytes[slab..slab + 32].to_vec();
        decipher(&mut bytes, slab, 24);
        // A slab's record holds the number of chunks and their size in
        // blocks; what does not (a stale record, enciphered under a key the
        // file left, deciphers to bytes that may start with 0xFB by chance)
        // is no slab.
        let (chunks, size) = (bytes[slab + 9] as usize, bytes[slab + 11] as usize);
        let genuine = bytes[slab + 8] == 0xFB
            && (1..=124).contains(&chunks)
            && (1..=32).contains(&size)
            && 4 + chunks * size <= 128;
        if !genuine {
            bytes[slab..slab + 32].copy_from_slice(&stored);
        } else {
            for chunk in 0..chunks {
                let in_use = bytes[slab + 16 + chunk / 8] >> (chunk % 8) & 1 != 0;
                let at = slab + (4 + chunk * size) * block;
                if !in_use && at + size * block <= bytes.len() {
                    bytes[at..at + size * block].fill(0);
                }
            }
            let (from, to) = (slab + (4 + chunks * size) * block, slab + 128 * block);
            if from < to && to <= bytes.len() {
                bytes[from..to].fill(0);
            }
        }
        slab += 128 * block;
    }
    bytes
}

/// `normalized`, and the rights granted to `DB:Public` blanked: the original
/// keeps them in the object with a frame of random bytes and a checksum of
/// them, which tflip cannot make, so it keeps the slot's bytes as they are.
fn normalized_add(add: &[u8]) -> Vec<u8> {
    let mut bytes = normalized(add, false);
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let (data, slots, record) = (word(&bytes, 0x20), word(&bytes, 0x18), word(&bytes, 0x24));
    let shift = record - 524;
    for slot in 0..slots {
        let base = data + slot * record;
        let masked = match &bytes[base + 13 + shift..base + 23 + shift] {
            kind if kind.starts_with(b"Table") => 12,
            kind if kind.starts_with(b"View") => 5,
            kind if kind.starts_with(b"StoredProc") => 6,
            kind if kind.starts_with(b"Function") => 0,
            // A user's membership of a built-in group is a blob with a
            // random salt that the original draws at each write.
            kind if kind.starts_with(b"User") => 8,
            _ => continue,
        };
        let property = base + 223 + shift;
        let (mut at, mut index) = (property, 0);
        while at + 2 <= property + 275 {
            let length = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            if length == 0xFFFF {
                at += 2;
                index += 1;
                continue;
            }
            let length = usize::from(length);
            if index == masked {
                bytes[at + 2..(at + 2 + length).min(property + 275)].fill(0);
            }
            at += 2 + length;
            index += 1;
        }
    }
    bytes
}

/// A copy of the dictionary `letter` of `before` in the folder `name` of `dir`.
fn copied(dir: &Path, letter: &str, name: &str) -> PathBuf {
    let work = dir.join(name);
    std::fs::create_dir_all(&work).unwrap();
    for entry in std::fs::read_dir(fixtures().join("before").join(letter)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), work.join(entry.file_name())).unwrap();
    }
    work
}

/// The users of a converted dictionary log in with the empty password and
/// not with the one they had (the cipher changed), the administrator with
/// his; one converted to the cipher it had keeps them.
#[test]
fn a_converted_dictionary_keeps_its_administrator_and_loses_the_users_passwords() {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-aes-converted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let session = Session::new();
    for (letter, target, kept) in [("P", "AES128", false), ("R", "RC4", true)] {
        let name = format!("login_{letter}");
        copied(&dir, letter, &name);
        let key = |user: &str, password: &str| {
            let aes = match target {
                "RC4" => String::new(),
                kind => format!("EncryptionType={kind};FIPS=FALSE;DDPassword=ddpw;"),
            };
            format!(
                "connect Data Source=%D\\{name}\\d.add;ServerType=LOCAL;User ID={user};\
                 Password={password};{aes}"
            )
        };
        let (works, fails) = match kept {
            true => ("userpw", ""),
            false => ("", "userpw"),
        };
        let script = format!(
            "connect Data Source=%D\\{name};ServerType=LOCAL;\n\
             sql EXECUTE PROCEDURE sp_SetDDEncryptionType('d.add', 'adm', 'ddpw', '{target}', 1)\n\
             disconnect\n\
             {admin}\nsql SELECT * FROM system.users\ndisconnect\n\
             {user}\nsql SELECT * FROM system.users\ndisconnect\n\
             {refused}\n",
            admin = key("ADSSYS", "adm"),
            user = key("u1", works),
            refused = key("u1", fails),
        );
        // The last connection, the 10th step, is the one refused.
        assert_eq!(
            run(&session, &dir, &script),
            vec![(10, 7078)],
            "{letter} to {target}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A dictionary with an encrypted table is not converted, and nothing is
/// written (the original: 5130, after the checks of the passwords).
#[test]
fn a_dictionary_with_an_encrypted_table_is_not_converted() {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-aes-refused-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let work = copied(&dir, "T", "refused");
    let before = std::fs::read(work.join("d.add")).unwrap();
    let session = Session::new();
    for (target, encrypt) in [("AES128", 1), ("RC4", 1), ("RC4", 0)] {
        let script = format!(
            "connect Data Source=%D\\refused;ServerType=LOCAL;\n\
             sql EXECUTE PROCEDURE sp_SetDDEncryptionType('d.add', 'adm', 'ddpw', '{target}', {encrypt})\n\
             disconnect\n"
        );
        assert_eq!(run(&session, &dir, &script), vec![(2, 7200)], "{target}");
        assert_eq!(std::fs::read(work.join("d.add")).unwrap(), before);
        assert!(!work.join("d.add.BAK").exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn converts_dictionaries_as_the_original() {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-aes-convert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let only = std::env::var("AES_ONLY").unwrap_or_default();
    let mut scripts: Vec<_> = std::fs::read_dir(scripts())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let stem = path.file_stem().unwrap().to_string_lossy();
            stem.starts_with("c_") || stem.starts_with("m_")
        })
        .collect();
    scripts.sort();
    let session = Session::new();
    let mut failures = Vec::new();
    for path in scripts {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if !name.contains(&only) {
            continue;
        }
        let work = dir.join(&name);
        std::fs::create_dir_all(&work).unwrap();
        let before = fixtures().join("before").join(source_of(&name));
        for entry in std::fs::read_dir(before).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), work.join(entry.file_name())).unwrap();
        }
        let failed = run(&session, &dir, &std::fs::read_to_string(&path).unwrap());
        if !failed.is_empty() {
            failures.push(format!("{name}: fails {failed:?}"));
            continue;
        }
        let theirs = fixtures().join("after").join(&name);
        let mut files: Vec<_> = [&work, &theirs]
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        files.dedup();
        let read = |root: &Path, file: &str| std::fs::read(root.join(file)).ok();
        for file in files {
            let (Some(a), Some(b)) = (read(&work, &file), read(&theirs, &file)) else {
                failures.push(format!("{name}/{file}: not in both"));
                continue;
            };
            // The file a BAK holds the layout of the name before it.
            let base = file.strip_suffix(".BAK").unwrap_or(&file);
            let add = |root: &Path| {
                let bak = if file.ends_with(".BAK") { ".BAK" } else { "" };
                read(root, &format!("d.add{bak}")).unwrap()
            };
            let (x, y) = (add(&work), add(&theirs));
            let (a, b) = match base.rsplit('.').next().unwrap() {
                "add" => (normalized_add(&a), normalized_add(&b)),
                "am" => (normalized_am(&a, &x), normalized_am(&b, &y)),
                "ai" => (
                    normalized_index(&a, &x, &dir),
                    normalized_index(&b, &y, &dir),
                ),
                _ => (a, b),
            };
            if let Some(difference) = first_difference(&a, &b) {
                failures.push(format!("{name}/{file}: {difference}"));
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

/// `sp_GetSecurityInfo` as the original answers it on each kind of
/// dictionary (`aesdict.exe`'s `query`): the encryption type of the
/// dictionary, and for its administrator alone whether it encrypts its
/// records; a user gets no answer to that.
#[test]
fn security_info_tells_the_administrator_whether_the_dictionary_is_encrypted() {
    use crate::script_host::tests::cursor_rows;
    use crate::{connection, statement, table};
    use tflip_storage::value::Value;

    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-aes-secinfo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let session = Session::new();
    let cases = [
        ("P", "ADSSYS", "adm", "RC4", Some(false)),
        ("P", "u1", "userpw", "RC4", None),
        ("R", "ADSSYS", "adm", "RC4", Some(true)),
        ("C", "ADSSYS", "adm", "AES128", Some(false)),
        ("E", "ADSSYS", "adm", "AES128", Some(true)),
        ("E", "u1", "userpw", "AES128", None),
        ("F", "ADSSYS", "adm", "AES256", Some(true)),
    ];
    for (letter, user, password, encryption, encrypted) in cases {
        let name = format!("{letter}_{user}");
        copied(&dir, letter, &name);
        let aes = match letter {
            "C" | "E" => "EncryptionType=AES128;FIPS=FALSE;DDPassword=ddpw;",
            "F" => "EncryptionType=AES256;FIPS=FALSE;DDPassword=ddpw;",
            _ => "",
        };
        let cs = format!(
            "Data Source={}\\{name}\\d.add;ServerType=LOCAL;User ID={user};Password={password};{aes}",
            dir.display()
        );
        let connection = connection::connect(&session, &cs.replace('\\', "/")).unwrap();
        let handle = statement::create(&session, connection).unwrap();
        let cursor = statement::execute_direct(
            &session,
            handle,
            "EXECUTE PROCEDURE sp_GetSecurityInfo()",
            true,
        )
        .unwrap()
        .unwrap();
        let (_, rows) = cursor_rows(&session, cursor);
        table::close(&session, cursor).unwrap();
        let row = &rows[0];
        assert_eq!(row[1], Value::Text(format!("{encryption:<10}")), "{name}");
        assert_eq!(
            row[2],
            encrypted.map_or(Value::Null, Value::Logical),
            "{name}"
        );
        connection::disconnect(&session, connection).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `sp_GetSecurityInfo`'s encryption type is the dictionary's when the
/// connection opened it: `sp_ModifyDatabase` of the type shows in the
/// connections after it (the original: `aesdict.exe`'s probe of a plain
/// dictionary given 5, then 6).
#[test]
fn security_info_keeps_the_type_the_connection_found() {
    use crate::script_host::tests::cursor_rows;
    use crate::{connection, statement, table};

    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-aes-sectype-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copied(&dir, "P", "t");
    let session = Session::new();
    let cs = format!(
        "Data Source={}/t/d.add;ServerType=LOCAL;User ID=ADSSYS;Password=adm;",
        dir.display()
    );
    let mut types = Vec::new();
    for modify in [Some(5), Some(6), None] {
        let connection = connection::connect(&session, &cs).unwrap();
        let handle = statement::create(&session, connection).unwrap();
        let ask = |sql: &str| {
            let cursor = statement::execute_direct(&session, handle, sql, true).unwrap();
            cursor.map(|cursor| {
                let (_, rows) = cursor_rows(&session, cursor);
                table::close(&session, cursor).unwrap();
                format!("{:?}", rows[0][1])
            })
        };
        types.push(ask("EXECUTE PROCEDURE sp_GetSecurityInfo()").unwrap());
        if let Some(kind) = modify {
            ask(&format!(
                "EXECUTE PROCEDURE sp_ModifyDatabase('DATA_ENCRYPTION_TYPE', '{kind}')"
            ));
            types.push(ask("EXECUTE PROCEDURE sp_GetSecurityInfo()").unwrap());
        }
        connection::disconnect(&session, connection).unwrap();
    }
    let name = |text: &str| format!("Text(\"{text:<10}\")");
    let (rc4, aes128, aes256) = (name("RC4"), name("AES128"), name("AES256"));
    assert_eq!(
        types,
        [rc4.clone(), rc4, aes128.clone(), aes128, aes256]
            .into_iter()
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
