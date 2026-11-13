//! `RAND([seed])`: the original engine's random numbers, learnt black-box
//! from its outputs (`tools/ace/sql/funcs.txt`).
//!
//! The generator is L'Écuyer's combined one: two multiplicative congruential
//! generators, `a' = 40014·a mod 2147483563` and `b' = 40692·b mod
//! 2147483399`, whose difference `z = a' − b'` (plus 2147483562 if it is not
//! positive) is the number, `z · 4.656613e-10`. The engine keeps only the low
//! 30 bits of each state before every step, which is why its sequence leaves
//! the textbook one after two numbers. A seed `s` (a 32-bit integer) starts
//! the states at `s` and `s + 1`, so 30 bits of it count; a seed of 0 is the
//! clock's milliseconds instead. Each connection has a generator of its own,
//! which starts at the states 1 and 69.

/// A connection's random-number generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rand {
    a: i64,
    b: i64,
}

const MASK: i64 = 0x3fff_ffff;
const M1: i64 = 2_147_483_563;
const A1: i64 = 40014;
const M2: i64 = 2_147_483_399;
const A2: i64 = 40692;

impl Default for Rand {
    /// The state a new connection's generator starts in.
    fn default() -> Rand {
        Rand { a: 1, b: 69 }
    }
}

impl Rand {
    /// Restarts the generator at `seed`, or at the clock for 0.
    pub fn seed(&mut self, seed: i32) {
        let seed = match seed {
            0 => clock_millis() as i32,
            s => s,
        };
        self.a = i64::from(seed) & MASK;
        self.b = i64::from(seed.wrapping_add(1)) & MASK;
    }

    /// The next number, in `(0, 1)`.
    pub fn draw(&mut self) -> f64 {
        self.a = (self.a & MASK) * A1 % M1;
        self.b = (self.b & MASK) * A2 % M2;
        let mut z = self.a - self.b;
        if z < 1 {
            z += M1 - 1;
        }
        z as f64 * 4.656613e-10
    }
}

/// A seed as the engine takes it from a number: truncated toward zero to a
/// 32-bit integer, and −2³¹ if it does not fit.
pub fn seed_of(x: f64) -> i32 {
    if x > -2_147_483_649.0 && x < 2_147_483_648.0 {
        x as i32
    } else {
        i32::MIN
    }
}

/// The milliseconds of the clock, as a tick count that wraps.
fn clock_millis() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's numbers as it shows them, with 15 decimals.
    fn shown(rand: &mut Rand, n: usize) -> Vec<String> {
        (0..n).map(|_| format!("{:.15}", rand.draw())).collect()
    }

    /// A new connection's first numbers, as the original engine gives them.
    #[test]
    fn a_new_generator_matches_the_engine() {
        assert_eq!(
            shown(&mut Rand::default(), 10),
            [
                "0.998711160597056",
                "0.542429629131165",
                "0.042266565595743",
                "0.368425970694504",
                "0.525732179997738",
                "0.196642216066832",
                "0.714089831243092",
                "0.106817272609242",
                "0.553059990225409",
                "0.070045905234282",
            ]
        );
    }

    /// Seeded: 1, and the 30 bits that count of −1 (whose second state is 0)
    /// and of 2³⁰ (whose first is).
    #[test]
    fn seeded_generators_match_the_engine() {
        let mut rand = Rand::default();
        rand.seed(1);
        assert_eq!(
            shown(&mut rand, 4),
            [
                "0.999980722801570",
                "0.203459467230847",
                "0.669574271045627",
                "0.772751560542384"
            ]
        );
        rand.seed(-1);
        assert_eq!(
            shown(&mut rand, 3),
            [
                "0.000773268307215",
                "0.941558414656657",
                "0.518076506594395"
            ]
        );
        rand.seed(1 << 30);
        assert_eq!(
            shown(&mut rand, 3),
            [
                "0.999981038519931",
                "0.228939909208787",
                "0.022882341565269"
            ]
        );
        rand.seed(i32::MAX);
        assert_eq!(shown(&mut rand, 1), ["0.000773268307215"]);
    }

    #[test]
    fn seeds_truncate_to_32_bits() {
        assert_eq!(seed_of(1.9), 1);
        assert_eq!(seed_of(-1.5), -1);
        assert_eq!(seed_of(2_147_483_647.5), i32::MAX);
        assert_eq!(seed_of(2_147_483_648.0), i32::MIN);
        assert_eq!(seed_of(-1e10), i32::MIN);
        assert_eq!(seed_of(f64::NAN), i32::MIN);
    }
}
