//! What `sp_SetDDEncryptionType` does to the files of a dictionary
//! (`fixtures/aes/convert`): the primitives it is made of.

use std::path::Path;

use crate::adt;
use crate::crypto::{self, OBFUSCATION_KEY};
use crate::memo::MemoFile;
use crate::table::{Error, RecordCipher, flags};

/// Enciphers (`on`) or deciphers the records and the memo values of the RC4
/// dictionary layout at `add` in place, as the original does when only the
/// flag of a dictionary of the default layout changes: each live record's
/// data under the fixed key at its slot (a deleted record is left as it is,
/// whichever way it was stored), each value those records refer to at the
/// slot of the record, the password check and the type in the header, and
/// the flag. The index is not touched: it is built anew.
pub fn set_rc4(add: &Path, on: bool) -> Result<(), Error> {
    let mut bytes = std::fs::read(add)?;
    let word = |bytes: &[u8], at: usize| {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    };
    if bytes.len() < adt::HEADER_SIZE || word(&bytes, 0x170) != 0 {
        return Err(Error::Corrupt("not a dictionary of the default layout"));
    }
    let (slots, data, length) = (word(&bytes, 0x18), word(&bytes, 0x20), word(&bytes, 0x24));
    let mut values = Vec::new();
    for slot in 1..=slots {
        let at = data + (slot - 1) * length;
        let record = bytes
            .get_mut(at..at + length)
            .ok_or(Error::Corrupt("dictionary shorter than its records"))?;
        let live = match on {
            true => adt::is_active_clear(record[0]),
            false => matches!(record[0], 0x02 | 0x06),
        };
        if !live {
            continue;
        }
        let body = adt::RECORD_PREFIX;
        if !on {
            crypto::table_record_xor(OBFUSCATION_KEY, slot as u32, &mut record[body..]);
        }
        for at in [super::create::MORE_PROPERTY, super::create::COMMENT] {
            let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
            let (block, stored) = (word(at), word(at + 4) as usize);
            if block != 0 && stored != 0 {
                values.push((slot as u32, block, stored));
            }
        }
        if on {
            crypto::table_record_xor(OBFUSCATION_KEY, slot as u32, &mut record[body..]);
        }
        record[0] = match on {
            true => record[0] | 0x02,
            false => record[0] & !0x02,
        };
    }
    if on {
        let check = crypto::rc4_password_check(OBFUSCATION_KEY);
        bytes[0x38..0x4C].copy_from_slice(&check);
    }
    let kind = if on {
        u32::from(adt::ENCRYPTION_RC4)
    } else {
        0
    };
    bytes[0x4C..0x50].copy_from_slice(&kind.to_le_bytes());
    let marks = word(&bytes, 0x58) as u32;
    let marks = if on {
        marks | flags::ENCRYPTED
    } else {
        marks & !flags::ENCRYPTED
    };
    bytes[0x58..0x5C].copy_from_slice(&marks.to_le_bytes());

    let mut memo = MemoFile::open_adm(&super::memo_path(add), true)?;
    for (slot, block, stored) in values {
        let mut value = memo.read_raw(block, stored)?;
        crypto::rc4_crypt(OBFUSCATION_KEY, slot, &mut value);
        memo.allocator()?.write(block, &value)?;
    }
    drop(memo);
    std::fs::write(add, bytes)?;
    Ok(())
}

/// Puts the AES dictionary at `add`, which encrypts its records, back in the
/// clear in place, as the original does first when it converts one
/// (`fixtures/aes/convert`): each live record (a deleted one is left as it is)
/// is written in the clear under a zero IV, the type and the flag cleared in
/// the header (the verifier and the crypto block stay), and the memo file
/// forgets its free space and takes each value, record by record, a `More
/// Property` before a `Comment`, in a place of its own in the clear, the
/// blocks it had freed. A connection of the process must have given the
/// dictionary's password.
pub fn decrypt_aes(add: &Path) -> Result<(), Error> {
    use std::io::{Seek, SeekFrom, Write};
    let Some(RecordCipher::Aes { key, nonce }) = super::keyring::cipher(add) else {
        return Err(Error::Encrypted);
    };
    let mut records = Vec::new();
    {
        let mut file = super::DictionaryFile::open(add)?;
        for slot in 1..=file.record_slots() {
            let record = file.record(slot)?;
            if adt::is_active_clear(record[0]) {
                records.push((slot, record));
            }
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(add)?;
    let mut head = [0u8; 0x5C];
    std::io::Read::read_exact(&mut file, &mut head)?;
    head[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
    let marks = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) & !flags::ENCRYPTED;
    head[0x58..0x5C].copy_from_slice(&marks.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)?;
    drop(file);

    let mut memo = MemoFile::open_adm(&super::memo_path(add), true)?;
    memo.set_iv_prefix(true);
    memo.set_enciphered_before(key.clone(), nonce);
    memo.allocator()?.forget_free_space()?;
    let mut file = super::DictionaryFile::open(add)?;
    for (slot, mut record) in records {
        for at in [super::create::MORE_PROPERTY, super::create::COMMENT] {
            let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
            let (block, stored) = (word(at), word(at + 4));
            if block == 0 || stored == 0 {
                continue;
            }
            let length = 8 + stored;
            let raw = memo.read_raw(block, length as usize)?;
            let iv: [u8; 8] = raw[..8].try_into().unwrap();
            let mut value = vec![0; 8];
            value.extend_from_slice(&raw[8..]);
            if iv != [0; 8] {
                crypto::aes_ctr_xor(&key, &iv, &nonce, &mut value[8..]);
            }
            let mut allocator = memo.allocator()?;
            let fresh = allocator.alloc(length)?;
            allocator.write(fresh, &value)?;
            allocator.free(block, length)?;
            record[at..at + 4].copy_from_slice(&fresh.to_le_bytes());
        }
        file.write(slot, &record)?;
    }
    Ok(())
}

/// Enciphers the AES dictionary at `add`, which keeps its records in the
/// clear, in place under the key of `password` and the type `kind` (5 or 6),
/// as the original does when a conversion ends in one that encrypts its
/// records (`fixtures/aes/convert`): the type and a new salt, and so a new key
/// and verifier, nonce and IVs, then each live record (a deleted one is left
/// as it is) and each memo value they refer to enciphered where it is, under
/// a fresh IV. What the memo file keeps besides the values (the slabs'
/// records, the free runs) stays in the clear. The index is not touched: it
/// is built anew.
pub fn encrypt_aes(add: &Path, password: &[u8], kind: u8) -> Result<(), Error> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let (records, values) = {
        let mut file = super::DictionaryFile::open(add)?;
        let mut memo = super::memo::DictMemo::open(&super::memo_path(add))?;
        let (mut records, mut values) = (Vec::new(), Vec::new());
        for slot in 1..=file.record_slots() {
            let record = file.record(slot)?;
            if !adt::is_active_clear(record[0]) {
                continue;
            }
            for at in [super::create::MORE_PROPERTY, super::create::COMMENT] {
                let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
                let (block, stored) = (word(at), word(at + 4) as usize);
                if block != 0 && stored != 0 {
                    values.push((block, memo.read_raw(block, stored)?));
                }
            }
            records.push((slot, record));
        }
        (records, values)
    };

    let material = crate::create::AesMaterial::generate(kind)?;
    let key_len = material.key_len();
    let derived = crypto::aes_derive(password, &material.salt, key_len);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(add)?;
    let mut head = vec![0u8; adt::HEADER_SIZE + 56];
    file.read_exact(&mut head)?;
    head[0x38..0x4C].copy_from_slice(&derived[key_len..key_len + 20]);
    head[0x4C..0x50].copy_from_slice(&u32::from(kind).to_le_bytes());
    let marks = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) | flags::ENCRYPTED;
    head[0x58..0x5C].copy_from_slice(&marks.to_le_bytes());
    let block = &mut head[adt::HEADER_SIZE..];
    block[0..4].copy_from_slice(&u32::from(kind).to_le_bytes());
    block[4..8].copy_from_slice(&material.nonce);
    block[8..16].copy_from_slice(&material.iv_counter.to_le_bytes());
    block[16..48].copy_from_slice(&material.salt);
    block[48..56].copy_from_slice(&material.descriptor_iv);
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)?;
    drop(file);

    super::keyring::forget(add);
    super::unlock(add, password).map_err(|_| Error::Encrypted)?;
    let mut file = super::DictionaryFile::open(add)?;
    for (slot, record) in &records {
        file.write(*slot, record)?;
    }
    drop(file);
    let mut memo = super::memo::DictMemo::open(&super::memo_path(add))?;
    for (block, value) in values {
        memo.write(block, &value)?;
    }
    Ok(())
}
