//! NChar and NVarChar keys: the ICU sort keys the original engine weighs
//! Unicode text by (its `:en_US` collation, ICU's root one, at tertiary
//! strength).
//!
//! A key is the text's primary weights, `0x01`, its secondary weights,
//! `0x01`, its tertiary weights. Each character stands for a list of
//! collation elements, which `icu_root.txt` holds for the characters and
//! contractions tflip knows (all but the ones below, and 50 it could not
//! work out, which fail): their primaries (one to four bytes), and their
//! secondary and tertiary weights. The table was worked out from the
//! original engine's keys, black-box, by `tools/ace/seek/nkeys.py`, and
//! checked against its keys of every BMP character, of the contractions of
//! UCA 5.1.0 (the version of ICU 4.0, the engine's) and of tens of thousands
//! of random strings (`fixtures/seek/nkeys.txt` holds some). What the table
//! leaves to formulas: the ideographs and every character no element is
//! defined for get ICU's implicit weights ([`implicit`]), the Hangul
//! syllables weigh as their jamo, a few characters have none, and a Thai or
//! Lao vowel written before its consonant sorts after it.
//!
//! The levels are written as ICU writes them. The common secondary and
//! tertiary weight (`0x05`) is compressed: a run of them before a higher
//! weight is one byte counting down from the level's top (`0x86`, `0x85`),
//! a run at the end one counting up from `0x05`, in chunks. Consecutive
//! primaries whose lead byte is a compressible one leave it out after the
//! first, and a change of lead after such a run is marked by `0xFF` (to a
//! higher lead) or `0x03` (a lower one).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// A level's compression: the byte a run before a higher weight counts down
/// from, and the longest runs a byte counts down (before a higher weight)
/// and up (at the end).
struct Level {
    top: u8,
    top_count: usize,
    bottom_count: usize,
}

const SECONDARY: Level = Level {
    top: 0x86,
    top_count: 0x40,
    bottom_count: 0x40,
};
const TERTIARY: Level = Level {
    top: 0x85,
    top_count: 0x54,
    bottom_count: 0x2B,
};

/// The common secondary and tertiary weight, which runs compress.
const COMMON: u8 = 0x05;

/// A character's collation elements.
#[derive(Clone)]
struct Elements {
    /// Each element's primary bytes (none for one without a primary).
    primaries: Vec<Vec<u8>>,
    secondaries: Vec<u8>,
    tertiaries: Vec<u8>,
}

struct Table {
    /// The lead bytes whose primaries are written without them after the
    /// first of a run.
    compressible: [bool; 256],
    chars: HashMap<char, Elements>,
    /// The contractions by their first character: the rest of the string
    /// and its elements.
    contractions: HashMap<char, Vec<(Vec<char>, Elements)>>,
    /// Characters whose elements tflip could not work out.
    unsupported: HashSet<char>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| parse(include_str!("icu_root.txt")))
}

fn parse(text: &str) -> Table {
    let hex = |word: &str| -> Vec<u8> {
        if word == "-" {
            return Vec::new();
        }
        (0..word.len() / 2)
            .map(|i| u8::from_str_radix(&word[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    };
    let mut table = Table {
        compressible: [false; 256],
        chars: HashMap::new(),
        contractions: HashMap::new(),
        unsupported: HashSet::new(),
    };
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words[0] == "compressible" {
            for word in &words[1..] {
                table.compressible[usize::from(hex(word)[0])] = true;
            }
            continue;
        }
        let chars: Vec<char> = words[0]
            .split('+')
            .filter_map(|code| char::from_u32(u32::from_str_radix(code, 16).unwrap()))
            .collect();
        if words[1] == "?" {
            table.unsupported.extend(chars);
            continue;
        }
        let primaries = match words[1] {
            "-" => Vec::new(),
            ces => ces.split('.').map(hex).collect(),
        };
        let elements = Elements {
            primaries,
            secondaries: hex(words[2]),
            tertiaries: hex(words[3]),
        };
        match chars[..] {
            [c] => {
                table.chars.insert(c, elements);
            }
            [first, ref rest @ ..] => table
                .contractions
                .entry(first)
                .or_default()
                .push((rest.to_vec(), elements)),
            [] => {}
        }
    }
    for list in table.contractions.values_mut() {
        list.sort_by_key(|(rest, _)| std::cmp::Reverse(rest.len()));
    }
    table
}

/// The Thai and Lao consonants that a vowel written before them sorts after.
const THAI: &[(u32, u32)] = &[(0x0E01, 0x0E2E)];
const LAO: &[(u32, u32)] = &[
    (0x0E81, 0x0E82),
    (0x0E84, 0x0E84),
    (0x0E87, 0x0E88),
    (0x0E8A, 0x0E8A),
    (0x0E8D, 0x0E8D),
    (0x0E94, 0x0E97),
    (0x0E99, 0x0E9F),
    (0x0EA1, 0x0EA3),
    (0x0EA5, 0x0EA5),
    (0x0EA7, 0x0EA7),
    (0x0EAA, 0x0EAB),
    (0x0EAD, 0x0EAE),
    (0x0EDC, 0x0EDD),
];

/// The consonants `vowel` sorts after when it is written before one.
fn consonants(vowel: char) -> &'static [(u32, u32)] {
    match vowel {
        '\u{0E40}'..='\u{0E44}' => THAI,
        '\u{0EC0}'..='\u{0EC4}' => LAO,
        _ => &[],
    }
}

/// The tertiary weight such a vowel has before its consonant.
const PREVOWEL_TERTIARY: u8 = 0xBD;

/// The primary weight of an ideograph, or of a character no collation
/// element is defined for, as ICU gives implicit weights: 3 bytes for the
/// ideographs of the two main blocks, 4 for the rest, from the character's
/// place in a sequence (probed, see `tools/ace/seek/nkeys.py`).
fn implicit(c: char) -> Vec<u8> {
    let cp = u32::from(c);
    if (0x3400..=0x4DBF).contains(&cp) || (0x4E00..=0x9FFF).contains(&cp) {
        // The main block, the 34 compatibility ideographs after it, the
        // extension A block; counted from 1.
        let n = 1 + match cp >= 0x4E00 {
            true => cp - 0x4E00,
            false => cp - 0x3400 + 0xA000 - 0x4E00 + 34,
        };
        return vec![
            0xE0 + (n / (125 * 251)) as u8,
            4 + (n / 125 % 251) as u8,
            4 + 2 * (n % 125) as u8,
        ];
    }
    let mut n = match (0x20000..=0x2A6D6).contains(&cp) {
        true => cp + 535_635,
        false => cp + 515_729 + 2 * 567_009,
    };
    let last = 4 + 28 * (n % 9) as u8;
    n /= 9;
    let third = 4 + (n % 251) as u8;
    n /= 251;
    let second = 4 + (n % 251) as u8;
    n /= 251;
    vec![0xE0 + n as u8, second, third, last]
}

/// Characters with no collation elements at all.
fn ignorable(c: char) -> bool {
    let cp = u32::from(c);
    (0xFDD0..=0xFDEF).contains(&cp) || cp & 0xFFFE == 0xFFFE
}

fn push(out: &mut Elements, e: &Elements) {
    out.primaries.extend(e.primaries.iter().cloned());
    out.secondaries.extend_from_slice(&e.secondaries);
    out.tertiaries.extend_from_slice(&e.tertiaries);
}

/// The collation elements of `text`, as lists of primaries, secondary
/// and tertiary weights; `None` if it has a character tflip does not know
/// them of.
fn elements(text: &str) -> Option<Elements> {
    let table = table();
    let chars: Vec<char> = text.chars().collect();
    let mut out = Elements {
        primaries: Vec::new(),
        secondaries: Vec::new(),
        tertiaries: Vec::new(),
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some((rest, e)) = table.contractions.get(&c).and_then(|list| {
            list.iter()
                .find(|(rest, _)| chars[i + 1..].starts_with(rest))
        }) {
            push(&mut out, e);
            i += 1 + rest.len();
            continue;
        }
        if table.unsupported.contains(&c) {
            return None;
        }
        // A Thai or Lao vowel written before its consonant sorts after it,
        // with a tertiary weight of its own.
        let next = chars.get(i + 1).copied();
        if let (Some(next), Some(vowel)) = (next, table.chars.get(&c))
            && consonants(c)
                .iter()
                .any(|&(lo, hi)| (lo..=hi).contains(&u32::from(next)))
            && let Some(consonant) = table.chars.get(&next)
        {
            push(&mut out, consonant);
            out.primaries.extend(vowel.primaries.iter().cloned());
            out.secondaries.extend_from_slice(&vowel.secondaries);
            out.tertiaries.extend(std::iter::repeat_n(
                PREVOWEL_TERTIARY,
                vowel.tertiaries.len(),
            ));
            i += 2;
            continue;
        }
        i += 1;
        let cp = u32::from(c);
        if let Some(e) = table.chars.get(&c) {
            push(&mut out, e);
        } else if (0xAC00..=0xD7A3).contains(&cp) {
            // A Hangul syllable weighs as its jamo.
            let s = cp - 0xAC00;
            let mut jamo = vec![0x1100 + s / 588, 0x1161 + s % 588 / 28];
            if s % 28 != 0 {
                jamo.push(0x11A7 + s % 28);
            }
            for j in jamo {
                push(&mut out, table.chars.get(&char::from_u32(j)?)?);
            }
        } else if !ignorable(c) {
            out.primaries.push(implicit(c));
            out.secondaries.push(COMMON);
            out.tertiaries.push(COMMON);
        }
    }
    Some(out)
}

/// The ICU sort key of `text` as the original engine builds it; `None` if
/// it has a character tflip does not know the collation elements of.
pub fn sort_key(text: &str) -> Option<Vec<u8>> {
    let table = table();
    let Elements {
        primaries,
        secondaries,
        tertiaries,
    } = elements(text)?;
    let mut key = Vec::new();
    let mut lead: Option<u8> = None;
    for primary in &primaries {
        if let Some(run) = lead {
            if primary[0] == run {
                key.extend_from_slice(&primary[1..]);
                continue;
            }
            key.push(if primary[0] > run { 0xFF } else { 0x03 });
            lead = None;
        }
        key.extend_from_slice(primary);
        if table.compressible[usize::from(primary[0])] {
            lead = Some(primary[0]);
        }
    }
    key.push(0x01);
    compress(&secondaries, &SECONDARY, &mut key);
    key.push(0x01);
    compress(&tertiaries, &TERTIARY, &mut key);
    Some(key)
}

/// Writes the weights of a level to `key`, its runs of common weights
/// compressed.
fn compress(weights: &[u8], level: &Level, key: &mut Vec<u8>) {
    let mut count = 0;
    let flush = |count: &mut usize, up: bool, key: &mut Vec<u8>| {
        if up {
            while *count > level.top_count {
                key.push(level.top - level.top_count as u8);
                *count -= level.top_count;
            }
            key.push(level.top - (*count - 1) as u8);
        } else {
            while *count > level.bottom_count {
                key.push(COMMON + level.bottom_count as u8);
                *count -= level.bottom_count;
            }
            key.push(COMMON + (*count - 1) as u8);
        }
        *count = 0;
    };
    for &weight in weights {
        if weight == COMMON {
            count += 1;
            continue;
        }
        if count > 0 {
            flush(&mut count, weight > COMMON, key);
        }
        key.push(weight);
    }
    if count > 0 {
        flush(&mut count, false, key);
    }
}

/// The key of an NChar or NVarChar value `text` in an order whose keys are
/// `key_length` bytes, as the original engine builds it: the sort key of the
/// text without its trailing spaces (unless it is all spaces), cut to the
/// key length or padded with zeros. `None` as for [`sort_key`].
pub fn nchar_key(text: &str, key_length: usize) -> Option<Vec<u8>> {
    let mut key = sort_key(nchar_text(text))?;
    key.resize(key_length, 0);
    Some(key)
}

/// The text an NChar or NVarChar key is built from: `text` without its
/// trailing spaces, unless it is all spaces (an NChar value of spaces keeps
/// them all).
pub fn nchar_text(text: &str) -> &str {
    match text.trim_end_matches(' ') {
        "" => text,
        trimmed => trimmed,
    }
}

/// The characters `UPPER` and `LOWER` of the original engine change, and what
/// they make of them (`icu_case.txt`).
fn case_table() -> &'static HashMap<char, (char, char)> {
    static TABLE: OnceLock<HashMap<char, (char, char)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let char_of = |hex: &str, same: char| match hex {
            "-" => same,
            hex => u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .unwrap_or(same),
        };
        include_str!("icu_case.txt")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let c = char::from_u32(u32::from_str_radix(words.next()?, 16).ok()?)?;
                let upper = char_of(words.next()?, c);
                let lower = char_of(words.next()?, c);
                Some((c, (upper, lower)))
            })
            .collect()
    })
}

/// `UPPER` of `text` as the original engine does it on Unicode text: a
/// character at a time, by the Unicode data of ICU 4.0 (ß stays, a final
/// sigma is not told from the others).
pub fn upper(text: &str) -> String {
    map_case(text, |&(upper, _)| upper)
}

/// [`upper`] for `LOWER`.
pub fn lower(text: &str) -> String {
    map_case(text, |&(_, lower)| lower)
}

fn map_case(text: &str, pick: impl Fn(&(char, char)) -> char) -> String {
    let table = case_table();
    text.chars()
        .map(|c| table.get(&c).map_or(c, &pick))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_follows_the_engine() {
        // Probed (`fixtures/write/nchar_expr`): a character at a time, so no
        // ß to SS and no final sigma, and İ goes to a plain i.
        assert_eq!(upper("aß\u{e9}\u{3c3}"), "A\u{df}\u{c9}\u{3a3}");
        assert_eq!(lower("\u{130}\u{3a3}\u{10400}"), "i\u{3c3}\u{10428}");
        // Letters Unicode 5.1 does not know are left as they are.
        assert_eq!(upper("\u{1c80}"), "\u{1c80}");
    }

    #[test]
    fn keys_match_the_engine() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/seek/nkeys.txt");
        let unhex = |text: &str| -> Vec<u8> {
            (0..text.len() / 2)
                .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
                .collect()
        };
        let mut checked = 0;
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let (text, key) = line.split_once(' ').unwrap();
            let units: Vec<u16> = unhex(text)
                .as_chunks()
                .0
                .iter()
                .map(|&u| u16::from_le_bytes(u))
                .collect();
            let text = format!("{:<12}", String::from_utf16(&units).unwrap());
            let key = unhex(key);
            assert_eq!(
                nchar_key(&text, key.len()).as_deref(),
                Some(&key[..]),
                "{text:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 3000);
    }

    #[test]
    fn keys_of_some_texts() {
        let hex = |text: &str| {
            sort_key(text).map(|k| k.iter().map(|b| format!("{b:02x}")).collect::<String>())
        };
        // From the adi_keys fixture's NChar order.
        assert_eq!(hex("abc").as_deref(), Some("2c2e3001070107"));
        assert_eq!(hex("Zz").as_deref(), Some("5e5e0106018f05"));
        assert_eq!(hex("Ωж€").as_deref(), Some("6070ff62500329680107018f06"));
        assert_eq!(hex("").as_deref(), Some("0101"));
        // Runs of common weights longer than a byte counts (probed).
        let long = sort_key(&"a".repeat(65)).unwrap();
        assert_eq!(&long[65..], &[0x01, 0x45, 0x05, 0x01, 0x30, 0x1a]);
        let long = sort_key(&("a".repeat(85) + "B")).unwrap();
        assert_eq!(&long[86..], &[0x01, 0x45, 0x1a, 0x01, 0x31, 0x85, 0x8f]);
        // Ideographs and unassigned characters by formula (probed).
        assert_eq!(hex("\u{4e00}").as_deref(), Some("e0040601050105"));
        // A character tflip could not work out.
        assert_eq!(sort_key("a\u{0660}"), None);
        assert_eq!(nchar_text("ab  "), "ab");
        assert_eq!(nchar_text("   "), "   ");
    }
}
