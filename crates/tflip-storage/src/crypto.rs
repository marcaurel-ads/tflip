//! Advantage record and dictionary encryption: RC4 with a per-item starting
//! IV, plus the dictionary's `AtGdQ` obfuscation and password-secret `wrap`.
//!
//! ## Provenance
//!
//! The cipher is textbook RC4 (public) keyed by the RC4 KSA over the password.
//! The one non-public piece is `F` — the function that produces each record's
//! RC4 starting counters `(i0, j0)` — and the dictionary's `AtGdQ`/`wrap`
//! constructions layered on the same primitive. These were obtained under an
//! explicit, scoped exception to the project's no-reverse-engineering rule
//! (encryption only), and every function here is verified exact against the
//! Advantage engine's own output (see `tools/ace/crypto` and the engine
//! round-trip tests below): the table `(i0,j0)` generator at 48000/48000
//! recovered records, and the dictionary `wrap`/`K` path against both a probe
//! dictionary and `fixtures/adt/dictionary/dict.add`.
//!
//! ## The generator `F`
//!
//! `F(S0, item)` seeds an L'Écuyer 1988 combined LCG from a nonlinear packing of
//! four consecutive bytes of the pristine RC4 permutation `S0`, with the item
//! number as the second component seed, and reads two scaled outputs as
//! `(i0, j0)`. `item` is the 1-based record number for a table, or the
//! construction-specific counter for the dictionary.

/// L'Écuyer 1988 combined-LCG constants.
const M1: i64 = 2_147_483_563;
const A1: i64 = 40_014;
const Q1: i64 = 53_668;
const R1: i64 = 12_211;
const M2: i64 = 2_147_483_399;
const A2: i64 = 40_692;
const Q2: i64 = 52_774;
const R2: i64 = 3_791;

/// Advantage truncates an encryption password to 20 bytes before the KSA.
pub const KEY_MAX: usize = 20;

/// The fixed obfuscation key the dictionary uses for its `AtGdQ` layer and as the
/// `crypt`/`wrap` fallback when a key is empty or begins with a NUL byte.
pub const OBFUSCATION_KEY: &[u8] = b"AtGdQ";

/// RC4 key schedule over `key` (the caller truncates to [`KEY_MAX`]).
pub fn rc4_ksa(key: &[u8]) -> [u8; 256] {
    let mut s = [0u8; 256];
    for (i, b) in s.iter_mut().enumerate() {
        *b = i as u8;
    }
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    s
}

/// One L'Écuyer step: advance the two states and return `u` in `(0, 1)`.
#[inline]
fn lecuyer(a: &mut i64, b: &mut i64) -> f64 {
    let k = *a / Q1;
    *a = A1 * (*a - k * Q1) - k * R1;
    if *a < 0 {
        *a += M1;
    }
    let k = *b / Q2;
    *b = A2 * (*b - k * Q2) - k * R2;
    if *b < 0 {
        *b += M2;
    }
    let mut z = *a - *b;
    if z < 1 {
        z += M1 - 1;
    }
    z as f64 * (1.0 / M1 as f64)
}

/// `F`: the RC4 starting counters `(i0, j0)` for `item` under permutation `s0`.
pub fn item_start(s0: &[u8; 256], item: u32) -> (u8, u8) {
    if item == 0 {
        return (0, 0);
    }
    let r = (item % 253) as usize;
    // Nonlinear packing of the four consecutive S0 bytes S0[r..r+3] (r ≤ 252, so
    // r+3 ≤ 255 stays in range — the reason the modulus is 253).
    let mut v: u32 = ((u32::from(s0[r]) << 16) | u32::from(s0[r + 2])) & 0x3F_FFFF;
    v |= (u32::from(s0[r + 1]) & 0x3FFF) << 8;
    v = (v << 8) | (u32::from(s0[r + 3]) & 0x3FFF_FFFF);
    let mut a = v as i64;
    let mut b = (item & 0x3FFF_FFFF) as i64;
    let i0 = (lecuyer(&mut a, &mut b) * 256.0) as u8;
    let j0 = (lecuyer(&mut a, &mut b) * 256.0) as u8;
    (i0, j0)
}

/// XOR `data` in place with the RC4 keystream of `s0` started at `(i0, j0)`.
fn apply_keystream(s0: &[u8; 256], i0: u8, j0: u8, data: &mut [u8]) {
    let mut s = *s0;
    let mut i = i0;
    let mut j = j0;
    for byte in data.iter_mut() {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[i as usize].wrapping_add(s[j as usize]);
        *byte ^= s[k as usize];
    }
}

/// RC4-(de)cipher `data` in place under `password` at `item` (RC4 is symmetric).
/// The password keys the schedule (truncated to [`KEY_MAX`]), and `item` — a
/// record number, `0x45 × field_count` for the descriptors, `0x45` for the
/// password check, a page number for an index — gives the starting state.
pub fn rc4_crypt(password: &[u8], item: u32, data: &mut [u8]) {
    let key = &password[..password.len().min(KEY_MAX)];
    let s0 = rc4_ksa(key);
    let (i0, j0) = item_start(&s0, item);
    apply_keystream(&s0, i0, j0, data);
}

/// Decrypt (or encrypt — RC4 is symmetric) a table record's data area in place,
/// given the raw encryption `password` and the 1-based `recno`. The caller passes
/// only the enciphered fixed-length data area, not the clear record prefix.
pub fn table_record_xor(password: &[u8], recno: u32, data: &mut [u8]) {
    rc4_crypt(password, recno, data);
}

/// The RC4 password check: the 20 bytes at header `0x38` are `"ADVANTAGE"`
/// enciphered as item `0x45`, so the right password deciphers them to it.
pub fn rc4_password_ok(password: &[u8], check: &[u8; 20]) -> bool {
    let mut value = *check;
    rc4_crypt(password, 0x45, &mut value);
    value.starts_with(b"ADVANTAGE")
}

/// The 20-byte password-check block the engine stores at header `0x38` when a
/// table is enabled for RC4 encryption: `b"ADVANTAGE"` with 11 trailing NUL
/// bytes, enciphered at item `0x45` under `password` (the caller truncates it to
/// the scheme's key length — 5 bytes for the 40-bit variant, 20 for the
/// default). The inverse of [`rc4_password_ok`]; verified byte-for-byte against
/// the engine's `AdsEnableEncryption`.
pub fn rc4_password_check(password: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 20];
    block[..9].copy_from_slice(b"ADVANTAGE");
    rc4_crypt(password, 0x45, &mut block);
    block
}

/// The password check a DBF table keeps in bytes 0x0C to 0x15 of its header
/// once it is set up for RC4 encryption: a 3 (4 with a memo file) and the word
/// `ADVANTAGE` (`advantage` with a memo file) enciphered at item 0x45, the
/// start of the stream the 20 bytes of an ADT table's check are
/// (`fixtures/aes/rc4tp`, `y3`).
pub fn dbf_password_check(password: &[u8], memo: bool) -> [u8; 10] {
    let mut block = [0u8; 10];
    block[0] = if memo { 4 } else { 3 };
    let word: &[u8; 9] = if memo { b"advantage" } else { b"ADVANTAGE" };
    block[1..].copy_from_slice(word);
    rc4_crypt(password, 0x45, &mut block[1..]);
    block
}

/// Whether `password` is the one a DBF table's `check` (see
/// [`dbf_password_check`]) was made with.
pub fn dbf_password_ok(password: &[u8], check: &[u8; 10]) -> bool {
    *check == dbf_password_check(password, check[0] == 4)
}

/// The cipher a dictionary enciphers its secrets with (passwords, the
/// dictionary key, group memberships, rights): RC4 in a dictionary of the
/// default encryption, AES-CTR in one whose `DATA_ENCRYPTION_TYPE` is AES-128
/// or AES-256. The constructions on top ([`crypt`], [`obfuscate`], [`wrap`])
/// and their item numbers are the same in both; AES keys on the key text cut
/// or zero-padded to 16 or 32 bytes, and starts its counter block at `item
/// (u64) ‖ item (u32)`, little-endian (`fixtures/aes/dict`, every secret of its
/// dictionaries checked).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    Rc4,
    Aes128,
    Aes256,
}

impl Scheme {
    /// The scheme of a dictionary whose `DATA_ENCRYPTION_TYPE` is
    /// `encryption_type` (5 and 6 are AES; 3, or none, RC4).
    pub fn of_encryption_type(encryption_type: Option<u32>) -> Scheme {
        match encryption_type {
            Some(5) => Scheme::Aes128,
            Some(6) => Scheme::Aes256,
            _ => Scheme::Rc4,
        }
    }

    /// XOR `data` with the keystream of `key` (already past the empty-key
    /// fallback) at `item`.
    fn apply(self, key: &[u8], item: u32, data: &mut [u8]) {
        let key_len = match self {
            Scheme::Rc4 => {
                let s0 = rc4_ksa(&key[..key.len().min(KEY_MAX)]);
                let (i0, j0) = item_start(&s0, item);
                apply_keystream(&s0, i0, j0, data);
                return;
            }
            Scheme::Aes128 => 16,
            Scheme::Aes256 => 32,
        };
        let mut aes_key = vec![0u8; key_len];
        let n = key.len().min(key_len);
        aes_key[..n].copy_from_slice(&key[..n]);
        let iv = u64::from(item).to_le_bytes();
        aes_ctr_xor(&aes_key, &iv, &item.to_le_bytes(), data);
    }
}

/// `key`, or [`OBFUSCATION_KEY`] in its place when it is empty or begins with
/// a NUL byte.
fn or_obfuscation_key(key: &[u8]) -> &[u8] {
    if key.is_empty() || key[0] == 0 {
        OBFUSCATION_KEY
    } else {
        key
    }
}

/// The dictionary `crypt(key, item, data)` primitive: XOR `data` in place with
/// the keystream of `key` (falling back to [`OBFUSCATION_KEY`] when empty or
/// when its first byte is NUL) at `item`, under `scheme`. For RC4 the key is
/// truncated to [`KEY_MAX`] and the stream starts at `F(S0, item)`.
pub fn crypt(scheme: Scheme, key: &[u8], item: u32, data: &mut [u8]) {
    scheme.apply(or_obfuscation_key(key), item, data);
}

/// Remove (or apply) the dictionary's `AtGdQ` obfuscation layer in place:
/// `crypt("AtGdQ", recno + slot, data)`.
pub fn obfuscate(scheme: Scheme, data: &mut [u8], recno: u32, slot: u32) {
    crypt(scheme, OBFUSCATION_KEY, recno.wrapping_add(slot), data);
}

/// The `wrap` item: `1 + Σ key[i]·(i+1)`, computed over the original key bytes.
fn wrap_item(key: &[u8]) -> u32 {
    let mut sum: u32 = 1;
    for (i, &b) in key.iter().enumerate() {
        sum = sum.wrapping_add(u32::from(b).wrapping_mul(i as u32 + 1));
    }
    sum
}

/// Dictionary `wrap(key, data)`: XOR `data` in place under `key`, with the item
/// derived from `key` by [`wrap_item`]. Note the argument order — the *key*
/// determines both the cipher's key and the item; the *data*'s length is the
/// output length. `wrap` is its own inverse for a fixed key.
pub fn wrap(scheme: Scheme, key: &[u8], data: &mut [u8]) {
    // The item stays the one computed from the original key, even when the
    // empty-key fallback replaces the key itself.
    scheme.apply(or_obfuscation_key(key), wrap_item(key), data);
}

// ---------------------------------------------------------------------------
// AES mode
//
// A newer encryption, self-contained in the table (no dictionary needed for the
// key): the key is PBKDF2-HMAC-SHA1 of the password over a random salt stored in
// the header, and records, descriptors, memo values and index pages are AES-CTR
// enciphered, each item with its own IV. Every constant here is verified against
// the engine's own fixtures (see the tests, and `fixtures/aes`).

/// The PBKDF2 iteration count the engine uses for an AES table's key.
pub const AES_ITERATIONS: u32 = 10_000;

/// Derive an AES table's key material: `PBKDF2-HMAC-SHA1(password, salt,
/// 10000)`, `key_len + 20` bytes long. The first `key_len` bytes (16 for
/// AES-128, 32 for AES-256) are the key; the next 20 are the verifier, which
/// equals the header's stored verifier when the password is right.
pub fn aes_derive(password: &[u8], salt: &[u8], key_len: usize) -> Vec<u8> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    // PBKDF2 is slow on purpose, and the same table or dictionary is opened
    // again and again: the process keeps what it derived (a few, as a
    // process uses few passwords).
    type Derived = HashMap<(Vec<u8>, Vec<u8>, usize), Vec<u8>>;
    static DERIVED: LazyLock<Mutex<Derived>> = LazyLock::new(Default::default);
    let entry = (password.to_vec(), salt.to_vec(), key_len);
    if let Some(out) = DERIVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&entry)
    {
        return out.clone();
    }
    let mut out = vec![0u8; key_len + 20];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, salt, AES_ITERATIONS, &mut out);
    let mut derived = DERIVED.lock().unwrap_or_else(|e| e.into_inner());
    if derived.len() >= 64 {
        derived.clear();
    }
    derived.insert(entry, out.clone());
    out
}

/// XOR `data` in place with the engine's AES-CTR keystream for one item. The
/// counter block is `iv (8) ‖ nonce (4) ‖ block (4, big-endian, from 1)`; the
/// keystream is `AES-ECB(key, counter_block)` for each successive block. `key`
/// is 16 bytes (AES-128) or 32 (AES-256).
pub fn aes_ctr_xor(key: &[u8], iv: &[u8; 8], nonce: &[u8; 4], data: &mut [u8]) {
    use aes::cipher::KeyInit;
    match key.len() {
        16 => aes_ctr_blocks(&aes::Aes128::new_from_slice(key).unwrap(), iv, nonce, data),
        32 => aes_ctr_blocks(&aes::Aes256::new_from_slice(key).unwrap(), iv, nonce, data),
        _ => {}
    }
}

fn aes_ctr_blocks<C>(cipher: &C, iv: &[u8; 8], nonce: &[u8; 4], data: &mut [u8])
where
    C: aes::cipher::BlockEncrypt<BlockSize = aes::cipher::consts::U16>,
{
    use aes::cipher::generic_array::GenericArray;
    for (index, chunk) in data.chunks_mut(16).enumerate() {
        let mut block = [0u8; 16];
        block[..8].copy_from_slice(iv);
        block[8..12].copy_from_slice(nonce);
        block[12..].copy_from_slice(&(index as u32 + 1).to_be_bytes());
        let mut state = GenericArray::clone_from_slice(&block);
        cipher.encrypt_block(&mut state);
        for (byte, k) in chunk.iter_mut().zip(state.iter()) {
            *byte ^= *k;
        }
    }
}

/// The outcome of decoding one `system.users` password secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSecret {
    /// Whether the supplied password verified against the stored check.
    pub password_ok: bool,
    /// The dictionary key `K` recovered from this user record.
    pub key: Vec<u8>,
}

/// Decode a `system.users` password secret: an `AtGdQ`-obfuscated
/// `20 | pwlen | 20` block (`first20 | middle | last20`). Returns whether
/// `password` verifies and the dictionary key `K = wrap(password, last20)`.
///
/// `blob` is the stored secret (without its length prefix), `recno` the user
/// object's record number and `slot` its obfuscation slot (0 for the password
/// property).
pub fn decode_user_secret(
    scheme: Scheme,
    blob: &[u8],
    recno: u32,
    slot: u32,
    password: &[u8],
) -> Option<UserSecret> {
    let mut deob = blob.to_vec();
    obfuscate(scheme, &mut deob, recno, slot);
    decode_secret(scheme, &deob, password)
}

/// Whether `password` authenticates against the user login secret `blob`
/// (obfuscated at the user's record `recno`, property slot 0). Used to log a
/// user in, as the original engine does: the password is matched exactly and
/// case-sensitively, and the empty password only authenticates a user whose own
/// password is empty.
///
/// The secret is `first20 | middle(pwlen) | last20`, so its length pins the
/// password length; we require an exact match before verifying. Without it the
/// empty password is degenerate — `decode_secret(_, b"")` verifies against any
/// secret — so it would log anyone in as any user.
pub fn user_password_ok(
    scheme: Scheme,
    blob: &[u8],
    recno: u32,
    slot: u32,
    password: &[u8],
) -> bool {
    blob.len() == 40 + password.len()
        && decode_user_secret(scheme, blob, recno, slot, password).is_some_and(|s| s.password_ok)
}

/// Decode a password secret that is already de-obfuscated: a `20 | pwlen | 20`
/// block `first20 | middle | last20`. The check is `wrap(wrap(password,
/// first20), middle) == password`; the key is `K = wrap(password, last20)`.
pub fn decode_secret(scheme: Scheme, deob: &[u8], password: &[u8]) -> Option<UserSecret> {
    let n = password.len();
    if deob.len() < 40 + n {
        return None;
    }
    let first20 = &deob[..20];
    let middle = &deob[20..20 + n];
    let last20 = &deob[20 + n..40 + n];

    let mut n_val = first20.to_vec();
    wrap(scheme, password, &mut n_val); // N = wrap(password, first20), 20 bytes
    let mut check = middle.to_vec();
    wrap(scheme, &n_val, &mut check); // wrap(N, middle), n bytes
    let password_ok = check == password;

    let mut key = last20.to_vec();
    wrap(scheme, password, &mut key); // K = wrap(password, last20)
    Some(UserSecret { password_ok, key })
}

/// Recover a dictionary's table password from the Database object's property
/// slot — already de-obfuscated — that holds `[L][table_pw wrapped with K][admin
/// password secret]`, given the admin password that unlocks `K`. The table
/// password is `wrap(K, table_pw_wrapped)`. Returns `None` if the admin password
/// does not verify.
pub fn dict_table_password(scheme: Scheme, slot: &[u8], admin_password: &[u8]) -> Option<Vec<u8>> {
    let length = *slot.first()? as usize;
    let wrapped = slot.get(1..1 + length)?;
    let secret = slot.get(1 + length..)?;
    let decoded = decode_secret(scheme, secret, admin_password)?;
    if !decoded.password_ok {
        return None;
    }
    let mut table_password = wrapped.to_vec();
    wrap(scheme, &decoded.key, &mut table_password);
    Some(table_password)
}

/// Recover the dictionary key `K` from the Database object's de-obfuscated
/// property slot `[L][table_pw wrapped with K][admin password secret]`, given
/// the admin password. `K` wraps the table password and users' group
/// memberships. `None` if the admin password does not verify.
pub fn dict_key(scheme: Scheme, slot: &[u8], admin_password: &[u8]) -> Option<Vec<u8>> {
    let length = *slot.first()? as usize;
    let secret = slot.get(1 + length..)?;
    // The secret is `first20 | middle(pwlen) | last20`, so its length pins the
    // password length. Require an exact match: the empty-password check is
    // otherwise degenerate (it verifies against any secret), which would hand
    // back a wrong key for a dictionary whose admin password is not empty.
    if secret.len() != 40 + admin_password.len() {
        return None;
    }
    let decoded = decode_secret(scheme, secret, admin_password)?;
    decoded.password_ok.then_some(decoded.key)
}

/// A user's group membership: XOR `data` under the dictionary key `key` with
/// [`wrap`], its own inverse. The cleartext is the member group object ids as
/// little-endian `u32`s, in membership order; the ciphertext is the user
/// object's property slot 1. Use to encode or decode (it is symmetric).
pub fn group_membership(scheme: Scheme, key: &[u8], data: &mut [u8]) {
    wrap(scheme, key, data);
}

/// The standard CRC-32 (IEEE 802.3, the zlib polynomial) of `bytes`.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The crypt item for a permission's property slot `slot` of the Permission
/// object whose own object id is `object_id`: `object_id + slot + 1`. The rights
/// blob is enciphered with [`crypt`] under the dictionary key at this item.
pub fn permission_item(object_id: u32, slot: u32) -> u32 {
    object_id + slot + 1
}

/// A permission's rights blob, before encipherment: the `rights` mask as a
/// little-endian `u32` followed by its CRC-32, the plaintext the original engine
/// keeps in a Permission object's property slot (then enciphered with [`crypt`]
/// under the dictionary key at [`permission_item`]).
pub fn permission_blob(rights: u32) -> [u8; 8] {
    let value = rights.to_le_bytes();
    let crc = crc32(&value).to_le_bytes();
    let mut blob = [0u8; 8];
    blob[..4].copy_from_slice(&value);
    blob[4..].copy_from_slice(&crc);
    blob
}

/// The cipher item of a user's built-in membership slot
/// ([`dd::user::BUILT_IN_MEMBERSHIP`](crate::dd::user::BUILT_IN_MEMBERSHIP)):
/// as a Permission object's slots', from the user's object id.
pub fn built_in_membership_item(user_id: u32) -> u32 {
    permission_item(user_id, crate::dd::user::BUILT_IN_MEMBERSHIP as u32)
}

/// What `x^32` is modulo the polynomial of the checksums below (the CRC-32
/// polynomial's reciprocal).
const CHECKSUM_POLY: u32 = 0xDB71_0641;

/// The product of `word` and `by` over GF(2) polynomials modulo
/// [`CHECKSUM_POLY`], with bit `i` of a little-endian word the coefficient of
/// `x^i`.
fn gf_multiply(word: u32, mut by: u32) -> u32 {
    let mut product = 0;
    for bit in 0..32 {
        if word >> bit & 1 != 0 {
            product ^= by;
        }
        by = (by << 1) ^ if by >> 31 != 0 { CHECKSUM_POLY } else { 0 };
    }
    product
}

/// The checksum of a built-in membership slot's plaintext: it is the random
/// word times `MEMBERSHIP_K` plus the mask times `K / x^64` plus a constant
/// (see [`gf_multiply`]). Found from the slots the original wrote
/// (`tools/ace/sql/ddbuiltin.txt`, 13 samples of 6 masks in three
/// dictionaries under different keys): it changes by exactly this under any
/// change of either, and the original reads no slot that does not check.
fn membership_checksum(salt: u32, mask: u32) -> u32 {
    const MEMBERSHIP_K: u32 = 0xCA19_4B8F;
    /// `MEMBERSHIP_K / x^64`.
    const MEMBERSHIP_MASK_K: u32 = 0x4B6B_15DC;
    const MEMBERSHIP_CONSTANT: u32 = 0xB3C3_C47F;
    gf_multiply(salt, MEMBERSHIP_K) ^ gf_multiply(mask, MEMBERSHIP_MASK_K) ^ MEMBERSHIP_CONSTANT
}

/// A user's built-in membership slot before encipherment (see
/// [`built_in_membership_item`]): 4 bytes the original draws at random on every
/// write (`salt`), a zero `u32`, the little-endian mask of the groups and the
/// checksum of the salt and the mask.
pub fn built_in_membership_blob(mask: u32, salt: [u8; 4]) -> [u8; 16] {
    let mut blob = [0u8; 16];
    blob[..4].copy_from_slice(&salt);
    blob[8..12].copy_from_slice(&mask.to_le_bytes());
    let check = membership_checksum(u32::from_le_bytes(salt), mask);
    blob[12..].copy_from_slice(&check.to_le_bytes());
    blob
}

/// The mask of a deciphered built-in membership `blob` (the inverse of
/// [`built_in_membership_blob`]): `None` unless it is 16 bytes that check, which
/// is how a wrong dictionary key is told from a right one.
pub fn built_in_membership_mask(blob: &[u8]) -> Option<u32> {
    let blob: &[u8; 16] = blob.try_into().ok()?;
    let word = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().unwrap());
    let mask = word(8);
    (word(4) == 0 && word(12) == membership_checksum(word(0), mask)).then_some(mask)
}

/// The checksum of a `DB:Public` rights slot's plaintext, of the same form as
/// [`membership_checksum`] with other coefficients: the random word times
/// `PUBLIC_K` plus the rights times `PUBLIC_RIGHTS_K` plus a constant. Found
/// from 32 slots of tables and views in dictionaries under different keys
/// (`tools/ace/dd/public.txt`): the 288 equations of the first nine fix the
/// three constants and the rest agree.
fn public_checksum(salt: u32, rights: u32) -> u32 {
    const PUBLIC_K: u32 = 0xDBA2_9DFE;
    const PUBLIC_RIGHTS_K: u32 = 0xB4A2_4BEC;
    const PUBLIC_CONSTANT: u32 = 0x30BB_E369;
    gf_multiply(salt, PUBLIC_K) ^ gf_multiply(rights, PUBLIC_RIGHTS_K) ^ PUBLIC_CONSTANT
}

/// The cipher item of the slot `slot` of an object with id `object_id` that
/// holds the rights granted to `DB:Public`: as a Permission object's slots'.
pub fn public_rights_item(object_id: u32, slot: u32) -> u32 {
    permission_item(object_id, slot)
}

/// The slot of the rights granted to `DB:Public` before encipherment (see
/// [`public_rights_item`]), 32 bytes: 4 the original draws at random on every
/// write (`salt`), a `u32` 1, two zero `u32`s, the little-endian rights, two
/// zero `u32`s and the checksum of the salt and the rights.
pub fn public_rights_blob(rights: u32, salt: [u8; 4]) -> [u8; 32] {
    let mut blob = [0u8; 32];
    blob[..4].copy_from_slice(&salt);
    blob[4] = 1;
    blob[16..20].copy_from_slice(&rights.to_le_bytes());
    let check = public_checksum(u32::from_le_bytes(salt), rights);
    blob[28..].copy_from_slice(&check.to_le_bytes());
    blob
}

/// The rights of a deciphered `DB:Public` slot `blob` (the inverse of
/// [`public_rights_blob`]): `None` unless it is 32 bytes that check, which is how
/// a wrong dictionary key is told from a right one.
pub fn public_rights(blob: &[u8]) -> Option<u32> {
    let blob: &[u8; 32] = blob.try_into().ok()?;
    let word = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().unwrap());
    let rights = word(16);
    let zeros = [8, 12, 20, 24].iter().all(|&at| word(at) == 0);
    (word(4) == 1 && zeros && word(28) == public_checksum(word(0), rights)).then_some(rights)
}

/// A column-rights blob before encipherment (the inverse of
/// [`permission_column_rights`]): the per-column right bytes followed by their
/// CRC-32, the plaintext a Permission object's column-rights slot holds.
pub fn permission_column_blob(columns: &[u8]) -> Vec<u8> {
    let crc = crc32(columns).to_le_bytes();
    columns.iter().copied().chain(crc).collect()
}

/// Encipher a replication subscription's secret (its password or connection
/// string) as the original engine stores it in the `Subscriptn` object's slot:
/// the text, a NUL, and the CRC-32 of (text + NUL), enciphered with [`crypt`]
/// under the dictionary key `key` at [`permission_item`]`(record, slot)` (the
/// subscription object's record number and the property slot). The inverse is
/// [`decipher_subscription_secret`].
pub fn encipher_subscription_secret(
    scheme: Scheme,
    key: &[u8],
    record: u32,
    slot: u32,
    text: &[u8],
) -> Vec<u8> {
    let mut blob = text.to_vec();
    blob.push(0);
    let crc = crc32(&blob).to_le_bytes();
    blob.extend_from_slice(&crc);
    crypt(scheme, key, permission_item(record, slot), &mut blob);
    blob
}

/// Decipher a subscription secret (see [`encipher_subscription_secret`]): the
/// text up to its NUL terminator, as the `Subscriptn` object's slot holds it.
pub fn decipher_subscription_secret(
    scheme: Scheme,
    key: &[u8],
    record: u32,
    slot: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut blob = data.to_vec();
    crypt(scheme, key, permission_item(record, slot), &mut blob);
    let end = blob.iter().position(|&b| b == 0).unwrap_or(blob.len());
    blob.truncate(end);
    blob
}

/// The rights of a deciphered permission `blob` (the inverse of
/// [`permission_blob`]): `None` if the trailing CRC-32 does not check, which is
/// how a wrong dictionary key is told from a right one.
pub fn permission_rights(blob: &[u8]) -> Option<u32> {
    if blob.len() != 8 {
        return None;
    }
    let value: [u8; 4] = blob[..4].try_into().unwrap();
    let crc = u32::from_le_bytes(blob[4..8].try_into().unwrap());
    (crc == crc32(&value)).then(|| u32::from_le_bytes(value))
}

/// The per-column rights of a deciphered column-rights `blob` (a Permission
/// object's [`crate::dd::permission::COLUMN_RIGHTS`] slot): one byte per field in
/// field order, followed by a CRC-32 of those bytes. `None` if the trailing
/// CRC-32 does not check (a wrong dictionary key), as [`permission_rights`].
pub fn permission_column_rights(blob: &[u8]) -> Option<Vec<u8>> {
    let value = blob.get(..blob.len().checked_sub(4)?)?;
    let crc = u32::from_le_bytes(blob[value.len()..].try_into().ok()?);
    (crc == crc32(value)).then(|| value.to_vec())
}

/// Build a password secret (the inverse of [`decode_secret`]): the
/// `first20 | middle | last20` block that verifies `password` and yields the
/// dictionary key `K = wrap(password, last20)`. `first20` and `last20` are the
/// caller's random salts; `middle` is `wrap(wrap(password, first20), password)`,
/// so the stored block is `password.len()` bytes longer than 40.
///
/// Verified against the original engine: a secret built here (with salts of our
/// own) authenticates where the engine's own secret does.
pub fn make_secret(
    scheme: Scheme,
    password: &[u8],
    first20: &[u8; 20],
    last20: &[u8; 20],
) -> Vec<u8> {
    let mut n_val = first20.to_vec();
    wrap(scheme, password, &mut n_val); // N = wrap(password, first20)
    let mut middle = password.to_vec();
    wrap(scheme, &n_val, &mut middle); // middle = wrap(N, password)
    let mut secret = Vec::with_capacity(40 + password.len());
    secret.extend_from_slice(first20);
    secret.extend_from_slice(&middle);
    secret.extend_from_slice(last20);
    secret
}

/// Build the Database object's property slot that holds the admin (`ADSSYS`)
/// login secret, the de-obfuscated form the writer stores after
/// [`obfuscate`]-ing it at the Database record (`recno` 1, slot 1). For a
/// dictionary that binds no encrypted table there is no wrapped table password,
/// so the leading length is zero and the slot is `[0x00] | make_secret(...)`.
/// `first20` and `last20` are the caller's random salts.
pub fn make_admin_slot(
    scheme: Scheme,
    password: &[u8],
    first20: &[u8; 20],
    last20: &[u8; 20],
) -> Vec<u8> {
    let mut slot = vec![0u8]; // L = 0: no wrapped table password
    slot.extend_from_slice(&make_secret(scheme, password, first20, last20));
    slot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_iv_generator_matches_engine() {
        // Verified against the Advantage engine (password "ABCD").
        let s0 = rc4_ksa(b"ABCD");
        assert_eq!(item_start(&s0, 1), (81, 73));
        assert_eq!(item_start(&s0, 2), (70, 180));
        assert_eq!(item_start(&s0, 0), (0, 0));
    }

    #[test]
    fn password_check_block_matches_engine() {
        // AdsEnableEncryption on a table with password "secret" writes this
        // 20-byte block at header 0x38 (scratchpad enc/encwrite.c, "enable").
        assert_eq!(
            &rc4_password_check(b"secret")[..],
            &hex(b"5c4298de9809ac4a1d5147fcaff7786ba89e93d2")[..],
        );
        // It is the inverse of the reader's check.
        assert!(rc4_password_ok(b"secret", &rc4_password_check(b"secret")));
        assert!(!rc4_password_ok(b"wrong", &rc4_password_check(b"secret")));
    }

    #[test]
    fn dictionary_crypt_matches_engine() {
        // crypt(Scheme::Rc4, "AtGdQ", 1, zeros) keystream head, verified against the engine's
        // encrypted-vs-plain dictionary obfuscation layer.
        let mut data = [0u8; 8];
        crypt(Scheme::Rc4, OBFUSCATION_KEY, 1, &mut data);
        assert_eq!(data, [0xa5, 0x61, 0xae, 0x73, 0x6e, 0x7d, 0x39, 0x4f]);
    }

    #[test]
    fn wrap_matches_engine_fixture() {
        assert_eq!(wrap_item(b"secret"), 2272);
        let mut data = hex(b"d598f908b3dcd9a46a58c312e5dd90cf79fce30b");
        wrap(Scheme::Rc4, b"secret", &mut data);
        assert_eq!(
            &data[..8],
            &[0x37, 0xc8, 0xe5, 0x97, 0x84, 0x01, 0x63, 0x74]
        );
    }

    #[test]
    fn public_rights_blob_matches_engine() {
        // Slots the original wrote for DB:Public (`tools/ace/dd/public.txt`),
        // deciphered: the random word, the rights and the checksum, in three
        // dictionaries under different keys.
        for (salt, rights, check) in [
            ("6ce6b16d", 0x1, "607968db"),
            ("1d76282d", 0x2, "eee135a2"),
            ("ddd34c31", 0x100, "9f96c001"),
            ("1c5eca19", 0x1, "b03bc39f"),
            ("7ba81ed1", 0x1, "e208767c"),
            ("0817fbff", 0x0, "e81aa069"),
            ("605aff08", 0x13, "3519b4bb"),
            ("ad008ae9", 0x33, "22a801e4"),
        ] {
            let salt: [u8; 4] = hex(salt.as_bytes()).try_into().unwrap();
            let blob = public_rights_blob(rights, salt);
            assert_eq!(&blob[28..], &hex(check.as_bytes())[..]);
            assert_eq!(public_rights(&blob), Some(rights));
            // Any byte changed is a slot that does not check.
            for at in 0..32 {
                let mut bad = blob;
                bad[at] ^= 1;
                assert_eq!(public_rights(&bad), None, "byte {at}");
            }
        }
        assert_eq!(public_rights(&[0; 31]), None);
    }

    #[test]
    fn made_secret_decodes_back() {
        // make_secret is the inverse of decode_secret: a secret built with our
        // own salts verifies the password and yields K = wrap(Scheme::Rc4, password, last20).
        // (The generation is also cross-checked against the original engine: a
        // dictionary written with such a secret authenticates ADSSYS.)
        let first20 = [0x11u8; 20];
        let last20 = [0x22u8; 20];
        for password in [&b""[..], b"a", b"abcd", b"password"] {
            let secret = make_secret(Scheme::Rc4, password, &first20, &last20);
            assert_eq!(secret.len(), 40 + password.len());
            let decoded = decode_secret(Scheme::Rc4, &secret, password).unwrap();
            assert!(decoded.password_ok);
            let mut k = last20.to_vec();
            wrap(Scheme::Rc4, password, &mut k);
            assert_eq!(decoded.key, k);
            // A different password of the same length must not verify.
            if !password.is_empty() {
                let wrong: Vec<u8> = password.iter().map(|b| b ^ 1).collect();
                assert!(
                    !decode_secret(Scheme::Rc4, &secret, &wrong)
                        .unwrap()
                        .password_ok
                );
            }
        }
    }

    #[test]
    fn user_password_ok_rejects_the_degenerate_empty_password() {
        // A user with a real password: the right password (obfuscated at the
        // user's record) authenticates, a wrong one and the empty one do not.
        let first20 = [0x11u8; 20];
        let last20 = [0x22u8; 20];
        let (recno, slot) = (7, 0);
        let mut blob = make_secret(Scheme::Rc4, b"secret", &first20, &last20);
        obfuscate(Scheme::Rc4, &mut blob, recno, slot); // store it as the record does
        assert!(user_password_ok(Scheme::Rc4, &blob, recno, slot, b"secret"));
        assert!(!user_password_ok(
            Scheme::Rc4,
            &blob,
            recno,
            slot,
            b"Secret"
        ));
        assert!(!user_password_ok(Scheme::Rc4, &blob, recno, slot, b"wrong"));
        // The empty password must NOT verify against a non-empty-password user,
        // though decode_secret alone would accept it (its check is empty-vs-empty).
        assert!(
            decode_user_secret(Scheme::Rc4, &blob, recno, slot, b"")
                .unwrap()
                .password_ok
        );
        assert!(!user_password_ok(Scheme::Rc4, &blob, recno, slot, b""));
        // A user whose own password is empty: only the empty password works.
        let mut empty = make_secret(Scheme::Rc4, b"", &first20, &last20);
        obfuscate(Scheme::Rc4, &mut empty, recno, slot);
        assert!(user_password_ok(Scheme::Rc4, &empty, recno, slot, b""));
        assert!(!user_password_ok(Scheme::Rc4, &empty, recno, slot, b"x"));
    }

    #[test]
    fn permission_rights_round_trip() {
        // permission_rights is the inverse of permission_blob, and the trailing
        // CRC-32 rejects a corrupted blob (a wrong dictionary key).
        for rights in [0x0u32, 0x1, 0x11, 0x333, 0x8000_0000, 0xffff_ffff] {
            let blob = permission_blob(rights);
            assert_eq!(permission_rights(&blob), Some(rights));
            let mut bad = blob;
            bad[0] ^= 1;
            assert_eq!(permission_rights(&bad), None);
        }
    }

    #[test]
    fn permission_column_rights_round_trip() {
        // Per-column grant bytes followed by their CRC-32, as a column-rights
        // slot holds; a corrupted byte fails the CRC.
        let columns = [0u8, 2, 0x11, 1];
        // permission_column_blob is the inverse of permission_column_rights.
        let blob = permission_column_blob(&columns);
        assert_eq!(
            permission_column_rights(&blob).as_deref(),
            Some(&columns[..])
        );
        let mut bad = blob.clone();
        bad[2] ^= 1;
        assert_eq!(permission_column_rights(&bad), None);
        // Too short to hold a CRC is rejected, not panicked on.
        assert_eq!(permission_column_rights(&[0, 1]), None);
    }

    #[test]
    fn admin_slot_round_trips_through_table_password_parser() {
        // The Database admin slot the writer stores (L = 0, no wrapped table
        // password) is parsed by the same code the reader uses; an empty default
        // ADSSYS password gives a 41-byte slot, as the engine writes.
        let first20 = [0x33u8; 20];
        let last20 = [0x44u8; 20];
        let slot = make_admin_slot(Scheme::Rc4, b"", &first20, &last20);
        assert_eq!(slot.len(), 41);
        assert_eq!(slot[0], 0);
        // With L = 0 there is no table password to recover, but the admin secret
        // after the length byte verifies the empty password.
        assert!(
            decode_secret(Scheme::Rc4, &slot[1..], b"")
                .unwrap()
                .password_ok
        );
    }

    #[test]
    fn wrap_is_its_own_inverse() {
        let mut data = *b"UserSecret01";
        let orig = data;
        wrap(Scheme::Rc4, b"somekey", &mut data);
        assert_ne!(data, orig);
        wrap(Scheme::Rc4, b"somekey", &mut data);
        assert_eq!(data, orig);
    }

    #[test]
    fn dict_table_password_matches_engine() {
        // fixtures/adt/dictionary_encrypted/rc4.add: the Database object is
        // record 1; its property slot 1 (after the record's AtGdQ decryption)
        // holds the table password wrapped under K and the ADSSYS secret. Admin
        // password "adssyspw" unlocks it to the table password "tblpass".
        let mut slot = hex(
            b"914ddc4a6c375896bdcceb33a56a3a2504a0e07b6bcd7ab1535dfb7b20927bc4fd850a857ce8cc85f8db08b6a2a86947d8b022eb1372755e",
        );
        obfuscate(Scheme::Rc4, &mut slot, 1, 1); // record 1, property slot 1 -> item 2
        assert_eq!(
            dict_table_password(Scheme::Rc4, &slot, b"adssyspw"),
            Some(b"tblpass".to_vec())
        );
        assert_eq!(dict_table_password(Scheme::Rc4, &slot, b"wrong"), None);
    }

    #[test]
    fn aes128_key_and_record_match_engine() {
        // fixtures/aes/aes128/s.adt, password "another pw".
        let salt = hex(b"c42a9e942ab9a04d4c94cafce028d7a4b08c36e8a264b1bc6041705d5e31d282");
        let derived = aes_derive(b"another pw", &salt, 16);
        assert_eq!(
            &derived[..16],
            &hex(b"b69930579b2ac7c311056c3656900ae7")[..],
            "key"
        );
        assert_eq!(
            &derived[16..36],
            &hex(b"2821c09e96f6851b979f67f601a08447f34f5a8a")[..],
            "verifier",
        );
        let nonce = [0xd6, 0x6e, 0x51, 0x80];
        let iv = [0x46, 0x64, 0x96, 0xc3, 0x3c, 0xb6, 0xee, 0x7d];
        let mut field_data = hex(b"519d76d5e5017a6587b167f86a21309786603d1d60d15a53e0");
        aes_ctr_xor(&derived[..16], &iv, &nonce, &mut field_data);
        assert_eq!(
            field_data,
            hex(b"01000000616c70686120202020202020810000000c00000000")
        );
    }

    #[test]
    fn aes256_key_matches_engine() {
        // fixtures/aes/aes256/m.adt, password "ddsecret".
        let salt = hex(b"912f6c88106b79ecbb562d849a266b1c7f919c433429ba17b62005d15be7ff47");
        let derived = aes_derive(b"ddsecret", &salt, 32);
        assert_eq!(
            &derived[..32],
            &hex(b"5eb93808c40f923d4da580d76206f4cc39b491a9c5d2ee409167511a4f029a5b")[..],
        );
        assert_eq!(
            &derived[32..52],
            &hex(b"694bc96c173fa7bdcfc73803480a54e7a55612d9")[..],
            "verifier",
        );
    }

    fn hex(h: &[u8]) -> Vec<u8> {
        h.chunks(2)
            .map(|c| {
                let d = |b: u8| match b {
                    b'0'..=b'9' => b - b'0',
                    b'a'..=b'f' => b - b'a' + 10,
                    _ => unreachable!(),
                };
                d(c[0]) << 4 | d(c[1])
            })
            .collect()
    }
}
