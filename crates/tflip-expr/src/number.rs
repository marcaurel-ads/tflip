//! Numbers as the original engine writes and reads them.

/// `value` with `decimals` decimals, as the original engine writes doubles
/// (`STR`): to 17 significant digits, the last decimal rounded half away
/// from zero; no sign for a value that rounds to zero.
pub fn fixed(value: f64, decimals: u16) -> String {
    rounded(value, decimals, 17)
}

/// `value` with `decimals` decimals, from its first `significant` digits,
/// the last decimal rounded half away from zero.
pub fn rounded(value: f64, decimals: u16, significant: usize) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    // The significant digits, and the exponent of the first.
    let scientific = format!("{:.*e}", significant - 1, value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|d| d - b'0')
        .collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    // Digit i is worth 10^(exponent - i); keep those down to 10^-decimals,
    // rounding on the next one.
    let decimals = i64::from(decimals);
    let kept = (exponent + 1 + decimals).max(0) as usize;
    let mut whole: Vec<u8> = (0..kept)
        .map(|i| digits.get(i).copied().unwrap_or(0))
        .collect();
    if digits.get(kept).is_some_and(|&d| d >= 5) {
        let mut i = whole.len();
        loop {
            if i == 0 {
                whole.insert(0, 1);
                break;
            }
            i -= 1;
            if whole[i] == 9 {
                whole[i] = 0;
            } else {
                whole[i] += 1;
                break;
            }
        }
    }
    // `whole` holds the value × 10^decimals.
    let point = whole.len() as i64 - decimals;
    let mut text = String::new();
    if value < 0.0 && whole.iter().any(|&d| d != 0) {
        text.push('-');
    }
    let digit = |i: i64| {
        if i >= 0 {
            whole.get(i as usize).map_or('0', |&d| char::from(b'0' + d))
        } else {
            '0'
        }
    };
    if point <= 0 {
        text.push('0');
    } else {
        (0..point).for_each(|i| text.push(digit(i)));
    }
    if decimals > 0 {
        text.push('.');
        (point..point + decimals).for_each(|i| text.push(digit(i)));
    }
    text
}

/// `STR(value, length, decimals)`: right-aligned in `length` characters,
/// or `length` asterisks if it does not fit.
pub fn str(value: f64, length: u16, decimals: u16) -> Vec<u8> {
    let text = fixed(value, decimals);
    let length = usize::from(length);
    if text.len() > length {
        return vec![b'*'; length];
    }
    format!("{text:>length$}").into_bytes()
}

/// `STRZERO(value, length, decimals)`: as [`str()`], padded with zeros after
/// the sign.
pub fn str_zero(value: f64, length: u16, decimals: u16) -> Vec<u8> {
    let text = str(value, length, decimals);
    if text.first() == Some(&b'*') {
        return text;
    }
    let negative = text.contains(&b'-');
    let digits: Vec<u8> = text
        .into_iter()
        .filter(|&c| c != b' ' && c != b'-')
        .collect();
    let mut padded = Vec::with_capacity(usize::from(length));
    if negative {
        padded.push(b'-');
    }
    padded.resize(usize::from(length) - digits.len(), b'0');
    padded.extend(digits);
    padded
}

/// Whether `value` fits a display format of `length` and `decimals`.
pub fn fits(value: f64, length: u16, decimals: u16) -> bool {
    fixed(value, decimals).len() <= usize::from(length)
}

/// The number `text` starts with, as C's `atof` reads it (spaces, a sign,
/// digits with a decimal point, an exponent); 0 if it starts with none.
pub fn atof(text: &str) -> f64 {
    let text = text.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let bytes = text.as_bytes();
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let whole = digits(end);
    end += whole;
    let mut fraction = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        end += 1 + fraction;
    }
    if whole + fraction == 0 {
        return 0.0;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits(end + 1 + sign);
        if exponent > 0 {
            end += 1 + sign + exponent;
        }
    }
    text[..end].parse().unwrap_or(0.0)
}

/// The number `text` starts with, as `VAL` reads it: as [`atof`], without
/// an exponent.
pub fn val(text: &str) -> f64 {
    let text = text.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let end = text.find(['e', 'E']).unwrap_or(text.len());
    atof(&text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_as_str_does() {
        assert_eq!(fixed(1.555, 2), "1.55");
        assert_eq!(fixed(1.565, 2), "1.56");
        assert_eq!(fixed(2.5, 0), "3");
        assert_eq!(fixed(-2.5, 0), "-3");
        assert_eq!(fixed(-0.0, 1), "0.0");
        assert_eq!(fixed(0.001, 2), "0.00");
        assert_eq!(fixed(-0.001, 2), "0.00");
        assert_eq!(fixed(9.995, 2), "9.99");
        assert_eq!(fixed(99.5, 0), "100");
        assert_eq!(fixed(123456789012345678.0, 0), "123456789012345680");
        assert_eq!(fixed(-9223372036854775807.0, 0), "-9223372036854775800");
        assert_eq!(fixed(1.5, 15), "1.500000000000000");
    }

    #[test]
    fn str_and_str_zero() {
        assert_eq!(str(1.5, 5, 0), b"    2");
        assert_eq!(str(123456.0, 3, 0), b"***");
        assert_eq!(str(1.5, 3, 5), b"***");
        assert_eq!(str_zero(-5.0, 4, 0), b"-005");
        assert_eq!(str_zero(-1.5, 6, 2), b"-01.50");
        assert_eq!(str_zero(5.0, 10, 0), b"0000000005");
        assert_eq!(str_zero(12345.0, 3, 0), b"***");
    }

    #[test]
    fn val_stops_before_an_exponent() {
        assert_eq!(val("  12.5abc"), 12.5);
        assert_eq!(val("1e3"), 1.0);
        assert_eq!(val("1,5"), 1.0);
        assert_eq!(val("abc"), 0.0);
        assert_eq!(atof("1e3"), 1000.0);
    }

    #[test]
    fn numbers_as_atof_reads_them() {
        for (text, value) in [
            ("1", 1.0),
            (" +1.5e1x", 15.0),
            ("1,5", 1.0),
            ("1e", 1.0),
            ("-.5", -0.5),
            ("0x10", 0.0),
            ("abc", 0.0),
            ("", 0.0),
            ("-", 0.0),
            ("nan", 0.0),
        ] {
            assert_eq!(atof(text), value, "{text}");
        }
        assert_eq!(atof("1e400"), f64::INFINITY);
    }
}
