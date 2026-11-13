//! The static ANSI collation the original engine falls back to when
//! `ANSI_CHAR_SET` is empty: one it derives from the host's (user) locale.
//!
//! Probed black-box (`tools/ace/collation/sysdefault.c`, under Wine and on
//! Windows 10, `fixtures/collation/sysdefault.json`):
//!
//! - If the user locale's language abbreviation (`LOCALE_SABBREVLANGNAME`) is
//!   `ENU`, `ENG` or `ENC` (English of the US, the UK or Canada), the
//!   collation is the default one, which weighs each byte as itself (what
//!   `ansi.chr` calls `Engl(Amer)`). Any other abbreviation, other English
//!   ones included, takes the next rule; so does a user locale whose
//!   abbreviation alone is overridden.
//! - Otherwise each byte's weight is its rank when the 256 one-byte strings
//!   are sorted by `CompareStringA` under the user locale, equal ones by their
//!   byte (`ä` after `a` and before `b` under German); its case tables are the
//!   locale's upper and lower case. `ansi.chr` plays no part: even under
//!   German the result is not its `German` collation.
//! - The system locale plays no part; nor does any of this touch the OEM
//!   collation, which stays the default with `OEM_CHAR_SET` empty.
//!
//! On Windows tflip asks Windows, as the original engine does. Elsewhere it
//! takes the locale from the environment (`LC_ALL`, else `LC_COLLATE`, else
//! `LANG`: `de_DE.UTF-8` is Windows' `de-DE`; `C`, `POSIX` or none is
//! `en-US`) and the collation from tables derived under Wine for every locale
//! Windows knows (`system_collations.bin`, made by
//! `tools/ace/collation/locales.c` and `gen_locales.py`).

use std::sync::OnceLock;

use tflip_storage::collation::Static;

/// The LCID of `en-US`, the locale of a host that names none.
pub const EN_US: u16 = 0x0409;

/// The static collations derived from Windows' locales, and the locales.
struct Tables {
    /// Each collation's weights, upper-case and lower-case bytes.
    tables: Vec<[[u8; 256]; 3]>,
    /// Each locale's LCID, name (`de-DE`) and collation (`None`: the default).
    locales: Vec<(u16, String, Option<usize>)>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| parse(include_bytes!("system_collations.bin")))
}

fn parse(bytes: &[u8]) -> Tables {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let count = usize::from(u16_at(0));
    let mut at = 2;
    let mut tables = Vec::with_capacity(count);
    for _ in 0..count {
        let table =
            |i: usize| -> [u8; 256] { bytes[at + i * 256..at + (i + 1) * 256].try_into().unwrap() };
        tables.push([table(0), table(1), table(2)]);
        at += 768;
    }
    let count = usize::from(u16_at(at));
    at += 2;
    let mut locales = Vec::with_capacity(count);
    for _ in 0..count {
        let lcid = u16_at(at);
        let slot = bytes[at + 2];
        let len = usize::from(bytes[at + 3]);
        let name = String::from_utf8_lossy(&bytes[at + 4..at + 4 + len]).into_owned();
        locales.push((lcid, name, (slot != 0xFF).then_some(usize::from(slot))));
        at += 4 + len;
    }
    Tables { tables, locales }
}

/// The collation (as derived under Wine) the original engine falls back to
/// under the Windows locale `lcid`; `None` for the default one, which weighs
/// each byte as itself (and for a locale Windows does not know).
pub fn for_lcid(lcid: u16) -> Option<Static> {
    let tables = tables();
    let (_, name, slot) = tables.locales.iter().find(|(l, ..)| *l == lcid)?;
    let [weights, upper, lower] = tables.tables[(*slot)?];
    Some(Static {
        name: name.clone(),
        weights,
        upper,
        lower,
    })
}

/// The LCID of the Windows locale a POSIX locale name (`de_DE.UTF-8`,
/// `sr_RS@latin`, `de`) stands for: the one of that name, else of that
/// language and country in any script, else the language's main one; `C`,
/// `POSIX`, an empty name or an unknown language is `en-US`.
pub fn lcid_of_posix(name: &str) -> u16 {
    let (name, modifier) = name.split_once('@').unwrap_or((name, ""));
    let name = name.split('.').next().unwrap_or("");
    let (language, country) = name.split_once(['_', '-']).unwrap_or((name, ""));
    if language.is_empty()
        || language.eq_ignore_ascii_case("C")
        || language.eq_ignore_ascii_case("POSIX")
    {
        return EN_US;
    }
    let script = match modifier.to_ascii_lowercase().as_str() {
        "latin" => "Latn",
        "cyrillic" => "Cyrl",
        "devanagari" => "Deva",
        _ => "",
    };
    let locales = &tables().locales;
    let parts = |n: &str| n.split('-').map(str::to_owned).collect::<Vec<_>>();
    let matches = |want: &[&str]| {
        locales.iter().find(|(_, n, _)| {
            let have = parts(n);
            have.len() == want.len()
                && have
                    .iter()
                    .zip(want)
                    .all(|(h, w)| h.eq_ignore_ascii_case(w))
        })
    };
    let exact = if script.is_empty() {
        matches(&[language, country])
    } else {
        matches(&[language, script, country])
    };
    let found = exact
        .or_else(|| {
            // The language and country, in whatever script.
            locales.iter().find(|(_, n, _)| {
                let have = parts(n);
                have.len() == 3
                    && have[0].eq_ignore_ascii_case(language)
                    && have[2].eq_ignore_ascii_case(country)
            })
        })
        .or_else(|| {
            // The language's main locale (sublanguage 1), else its first.
            let of_language = |(_, n, _): &&(u16, String, Option<usize>)| {
                parts(n)[0].eq_ignore_ascii_case(language)
            };
            locales
                .iter()
                .filter(of_language)
                .find(|(l, ..)| l >> 10 == 1)
                .or_else(|| locales.iter().find(of_language))
        });
    found.map_or(EN_US, |(lcid, ..)| *lcid)
}

/// The POSIX locale collation is taken from: `LC_ALL`, else `LC_COLLATE`,
/// else `LANG` (the first of them set and not empty).
pub fn posix_locale() -> String {
    ["LC_ALL", "LC_COLLATE", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// The static ANSI collation the original engine would fall back to on this
/// host with `ANSI_CHAR_SET` empty; `None` for the default one. Taken once
/// per process, as the original engine's local server does.
pub fn host() -> Option<Static> {
    static HOST: OnceLock<Option<Static>> = OnceLock::new();
    HOST.get_or_init(|| {
        #[cfg(windows)]
        {
            tflip_storage::collation::user_locale_default()
        }
        #[cfg(not(windows))]
        {
            for_lcid(lcid_of_posix(&posix_locale()))
        }
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The locale names Wine made of each probed POSIX locale.
    #[test]
    fn posix_names_map_to_windows_locales() {
        for (posix, lcid) in [
            ("de_DE.UTF-8", 0x0407),
            ("de_AT.utf8", 0x0c07),
            ("en_US.UTF-8", 0x0409),
            ("en_GB", 0x0809),
            ("sv_SE.ISO-8859-1", 0x041d),
            ("C", 0x0409),
            ("POSIX", 0x0409),
            ("", 0x0409),
            ("C.UTF-8", 0x0409),
            ("de", 0x0407),
            ("sr_RS@latin", 0x241a),
            ("xx_YY", 0x0409),
            ("zh_TW.UTF-8", 0x0404),
        ] {
            assert_eq!(lcid_of_posix(posix), lcid, "{posix}");
        }
    }

    /// What the original engine did under each host locale with
    /// `ANSI_CHAR_SET` empty: the default collation under US, UK and Canadian
    /// English, else the one Windows' sorting derives — byte for byte its ADI
    /// keys, its NTX order, the collation CRC its ADI files record, and how
    /// `UPPER`/`LOWER` fold case (the case byte weighed); OEM tables keep the
    /// default.
    #[test]
    fn system_collations_match_the_engine() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/collation/sysdefault.json"
        ))
        .unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
        let identity: Vec<u8> = (1..=255).collect();
        let mut derived = 0;
        for run in fixture["runs"].as_array().unwrap() {
            let locale = run["locale"].as_str().unwrap();
            let lcid = u16::from_str_radix(run["windows"]["user"].as_str().unwrap(), 16).unwrap();
            assert_eq!(lcid_of_posix(locale), lcid, "{locale}");
            let collation = for_lcid(lcid);
            let weights = collation
                .as_ref()
                .map_or(std::array::from_fn(|b| b as u8), |c| c.weights);
            let got: Vec<u8> = (1..=255).map(|b| weights[b]).collect();
            assert_eq!(
                got,
                hex(run["weights"].as_str().unwrap()),
                "{locale} weights"
            );
            let mut order: Vec<u8> = (1..=255).collect();
            order.sort_by_key(|&b| (weights[usize::from(b)], b));
            assert_eq!(order, hex(run["ansi"].as_str().unwrap()), "{locale} order");
            assert_eq!(hex(run["oem"].as_str().unwrap()), identity, "{locale} oem");
            let crc = tflip_storage::create::collation_crc(collation.as_ref().map(|c| &c.weights));
            assert_eq!(u64::from(crc), run["crc"].as_u64().unwrap(), "{locale} crc");
            for (kind, field) in [("upper", 0), ("lower", 1)] {
                let folded: Vec<u8> = (1..=255usize)
                    .map(|b| match &collation {
                        Some(c) => c.weights[usize::from([c.upper, c.lower][field][b])],
                        None => match field {
                            0 => (b as u8).to_ascii_uppercase(),
                            _ => (b as u8).to_ascii_lowercase(),
                        },
                    })
                    .collect();
                assert_eq!(folded, hex(run[kind].as_str().unwrap()), "{locale} {kind}");
            }
            derived += usize::from(collation.is_some());
        }
        assert!(derived >= 15, "{derived}");
    }

    /// The tables derived under Wine are those native Windows 10 derived for
    /// the user locale (the system one plays no part, and a user locale that
    /// claims to be `ENU` keeps the default).
    #[test]
    fn native_windows_derives_the_same_collations() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/collation/sysdefault.json"
        ))
        .unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
        let expected = [Some(0x0409), Some(0x0407), Some(0x041d), Some(0x041d), None];
        let runs = fixture["native"].as_array().unwrap();
        assert_eq!(runs.len(), expected.len());
        for (run, lcid) in runs.iter().zip(expected) {
            let collation = lcid.and_then(for_lcid);
            let crc = tflip_storage::create::collation_crc(collation.as_ref().map(|c| &c.weights));
            assert_eq!(
                u64::from(crc),
                run["crc"].as_u64().unwrap(),
                "{}",
                run["locale"]
            );
        }
    }

    #[test]
    fn the_tables_cover_windows_locales() {
        let tables = tables();
        assert!(tables.locales.len() > 200);
        assert!(
            for_lcid(0x0409).is_none() && for_lcid(0x0809).is_none() && for_lcid(0x1009).is_none()
        );
        assert!(for_lcid(0x0c09).is_some(), "en-AU derives one");
        let german = for_lcid(0x0407).unwrap();
        assert_eq!(german.name, "de-DE");
        // ä weighs just after a, before b; Ä is ä's upper case.
        let w = |b: u8| german.weights[usize::from(b)];
        assert!(w(b'a') < w(0xE4) && w(0xE4) < w(b'b'));
        assert_eq!(german.upper[0xE4], 0xC4);
        assert!(for_lcid(0x7FFF).is_none());
    }
}
