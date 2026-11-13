//! A data dictionary's memo file (`.am`), open to write.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use crate::adt::AES_IV_SIZE;
use crate::memo::MemoFile;
use crate::table::{Error, RecordCipher, flags};

/// The values of an RC4 dictionary's memo file that were written and no
/// record refers to yet: in the clear until the record is stored in its slot
/// ([`encipher_pending`]), as the slot is what enciphers them.
static PENDING: LazyLock<Mutex<HashSet<(PathBuf, u32)>>> = LazyLock::new(Default::default);

fn pending() -> MutexGuard<'static, HashSet<(PathBuf, u32)>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The slot of the live record of the dictionary `add` whose `Comment` or
/// `More Property` refers to `block`, the item an RC4 dictionary enciphers
/// its value at (`fixtures/aes/dictwrite`, `22_rc4_create`).
fn owner(add: &Path, block: u32) -> Result<Option<u32>, Error> {
    let mut file = super::DictionaryFile::open(add)?;
    for slot in 1..=file.record_slots() {
        let record = file.record(slot)?;
        let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
        let live = crate::adt::is_active_clear(record[0] & !crate::adt::TX_MARK);
        if live
            && (word(super::create::COMMENT) == block
                || word(super::create::MORE_PROPERTY) == block)
        {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}

/// Enciphers in place the values the just stored `record`, in `slot` of the
/// RC4 dictionary `add`, refers to and that were written before, in the clear.
pub(crate) fn encipher_pending(add: &Path, slot: u32, record: &[u8]) -> Result<(), Error> {
    let memo_path = super::memo_path(add);
    let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
    for at in [super::create::MORE_PROPERTY, super::create::COMMENT] {
        let (block, length) = (word(at), word(at + 4));
        if block == 0 || !pending().remove(&(memo_path.clone(), block)) {
            continue;
        }
        let mut memo = DictMemo::open(&memo_path)?;
        let mut value = memo.memo.read_raw(block, length as usize)?;
        crate::crypto::rc4_crypt(crate::crypto::OBFUSCATION_KEY, slot, &mut value);
        memo.memo.allocator()?.write(block, &value)?;
    }
    Ok(())
}

/// The memo file of a dictionary, its values written and read as the
/// dictionary's: an AES dictionary stores each behind an 8-byte IV, which is
/// zero and the value clear when it keeps its records in the clear, and
/// fresh, the value enciphered to the end of its blocks, when it encrypts
/// them (`fixtures/aes/dictwrite`); an RC4 dictionary's are enciphered at the
/// slot of the record that refers to each, to their length. The lengths it takes and gives are the
/// values' own, as the records refer to them; the IV takes 8 bytes more of
/// the blocks. It offers what [`crate::adm::Allocator`] does, by the same
/// names.
pub(crate) struct DictMemo {
    memo: MemoFile,
    /// The bytes before each value: the IV of an AES dictionary's.
    overhead: u32,
    /// How a dictionary that encrypts its records enciphers the values.
    cipher: Option<Enciphering>,
    /// The `.add` of a dictionary that encrypts its records with RC4.
    rc4: Option<PathBuf>,
}

/// The key, nonce and IV counter of a dictionary that encrypts its records,
/// and its `.add`, whose header keeps the counter.
struct Enciphering {
    key: Vec<u8>,
    nonce: [u8; 4],
    counter: Arc<AtomicU64>,
    add: PathBuf,
}

impl DictMemo {
    /// Opens the memo file at `memo_path`, of the dictionary beside it.
    pub(crate) fn open(memo_path: &Path) -> Result<DictMemo, Error> {
        let mut memo = MemoFile::open_adm(memo_path, true)?;
        let add = dictionary_of(memo_path);
        let mut head = [0u8; 0x174];
        let read = std::fs::File::open(&add).and_then(|mut file| {
            use std::io::Read;
            file.read_exact(&mut head)
        });
        let aes = read.is_ok() && u32::from_le_bytes(head[0x170..0x174].try_into().unwrap()) != 0;
        let encrypted = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) & flags::ENCRYPTED;
        let rc4 =
            (read.is_ok() && !aes && encrypted != 0 && head[0x4C] == crate::adt::ENCRYPTION_RC4)
                .then(|| add.clone());
        let cipher = match aes && encrypted != 0 {
            false => None,
            true => match super::keyring::reserve(&add)?.ok_or(Error::Encrypted)? {
                (RecordCipher::Aes { key, nonce }, counter) => Some(Enciphering {
                    key,
                    nonce,
                    counter,
                    add,
                }),
                _ => return Err(Error::Encrypted),
            },
        };
        // The allocator stores its slabs' records as the values are stored.
        memo.set_iv_prefix(aes);
        if let Some(cipher) = &cipher {
            memo.set_cipher(RecordCipher::Aes {
                key: cipher.key.clone(),
                nonce: cipher.nonce,
            });
            memo.set_aes_counter(Arc::clone(&cipher.counter));
        }
        Ok(DictMemo {
            memo,
            overhead: if aes { AES_IV_SIZE as u32 } else { 0 },
            cipher,
            rc4,
        })
    }

    /// The memo file of the dictionary `add`.
    fn memo_path(&self, add: &Path) -> PathBuf {
        super::memo_path(add)
    }

    /// The blocks a value of `length` bytes takes.
    pub(crate) fn blocks(&self, length: u32) -> u32 {
        (length + self.overhead)
            .div_ceil(self.memo.block_size())
            .max(1)
    }

    /// Space for a value of `length` bytes: its first block.
    pub(crate) fn alloc(&mut self, length: u32) -> Result<u32, Error> {
        let overhead = self.overhead;
        self.memo.allocator()?.alloc(length + overhead)
    }

    /// Frees the value of `length` bytes at `block`.
    pub(crate) fn free(&mut self, block: u32, length: u32) -> Result<(), Error> {
        let overhead = self.overhead;
        self.memo.allocator()?.free(block, length + overhead)
    }

    /// Counts one more allocation of the file, as the original engine does
    /// for some it makes (see [`crate::adm::Allocator::count_allocation`]).
    pub(crate) fn count_allocation(&mut self) -> Result<(), Error> {
        Ok(self.memo.allocator()?.count_allocation()?)
    }

    /// Writes the value `data` at block `block`.
    pub(crate) fn write(&mut self, block: u32, data: &[u8]) -> Result<(), Error> {
        let mut stored = Vec::with_capacity(data.len() + self.overhead as usize);
        if let Some(add) = &self.rc4 {
            // A value no record refers to yet waits in the clear for its slot.
            let mut value = data.to_vec();
            match owner(add, block)? {
                Some(slot) => {
                    crate::crypto::rc4_crypt(crate::crypto::OBFUSCATION_KEY, slot, &mut value)
                }
                None => {
                    pending().insert((self.memo_path(add), block));
                }
            }
            self.memo.allocator()?.write(block, &value)?;
            return Ok(());
        }
        match &self.cipher {
            None => {
                stored.resize(self.overhead as usize, 0);
                stored.extend_from_slice(data);
            }
            // Enciphered to the end of its blocks, the rest zeros, as the
            // original stores it.
            Some(cipher) => {
                let iv = cipher.counter.fetch_add(1, Ordering::Relaxed).to_le_bytes();
                let room = self.blocks(data.len() as u32) * self.memo.block_size();
                let mut value = data.to_vec();
                value.resize(room as usize - AES_IV_SIZE, 0);
                crate::crypto::aes_ctr_xor(&cipher.key, &iv, &cipher.nonce, &mut value);
                stored.extend_from_slice(&iv);
                stored.extend_from_slice(&value);
            }
        }
        self.memo.allocator()?.write(block, &stored)?;
        if let Some(cipher) = &self.cipher {
            super::keyring::persist(&cipher.add, &cipher.counter)?;
        }
        Ok(())
    }

    /// The value of `length` bytes at `block`.
    pub(crate) fn read_raw(&mut self, block: u32, length: usize) -> Result<Vec<u8>, Error> {
        if let Some(add) = &self.rc4 {
            let mut value = self.memo.read_raw(block, length)?;
            if !pending().contains(&(self.memo_path(add), block))
                && let Some(slot) = owner(add, block)?
            {
                crate::crypto::rc4_crypt(crate::crypto::OBFUSCATION_KEY, slot, &mut value);
            }
            return Ok(value);
        }
        let overhead = self.overhead as usize;
        let stored = self.memo.read_raw(block, length + overhead)?;
        let mut value = stored[overhead..].to_vec();
        if let Some(cipher) = &self.cipher {
            let iv: [u8; 8] = stored[..AES_IV_SIZE].try_into().unwrap();
            crate::crypto::aes_ctr_xor(&cipher.key, &iv, &cipher.nonce, &mut value);
        }
        Ok(value)
    }
}

/// The dictionary whose memo file is at `memo_path`: the same name with
/// `.add` (`.ADD` if the memo's extension is upper case).
fn dictionary_of(memo_path: &Path) -> PathBuf {
    let upper = memo_path
        .extension()
        .is_some_and(|e| e.to_string_lossy().chars().all(|c| c.is_ascii_uppercase()));
    memo_path.with_extension(if upper { "ADD" } else { "add" })
}
