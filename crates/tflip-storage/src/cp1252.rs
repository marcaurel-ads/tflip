//! Windows-1252, the character set of ADT field names and text.

/// The characters of bytes 0x80–0x9F; the rest of Windows-1252 is Latin-1.
/// The five bytes Windows-1252 leaves undefined map to the C1 control
/// characters of the same value, as Windows does.
const HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
];

/// Decodes Windows-1252 bytes.
pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

/// Encodes text as Windows-1252, the reverse of [`decode`]; characters it
/// cannot represent become `?`.
pub fn encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match u32::from(c) {
            0x80..=0x9F | 0x100.. => match HIGH.iter().position(|&h| h == c) {
                Some(i) => 0x80 + i as u8,
                None => b'?',
            },
            code => code as u8,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes() {
        assert_eq!(decode(b"abc"), "abc");
        assert_eq!(decode(&[0x80, 0x9F, 0xC4, 0xFC, 0xDF]), "€ŸÄüß");
        assert_eq!(decode(&[0x81]), "\u{81}");
    }

    #[test]
    fn encodes() {
        assert_eq!(
            encode("abc€ŸÄüß\u{81}"),
            [b'a', b'b', b'c', 0x80, 0x9F, 0xC4, 0xFC, 0xDF, 0x81]
        );
        assert_eq!(encode("\u{80}Ωж"), b"???");
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(encode(&decode(&all)), all);
    }
}
