//! Full-text search: the `CONTAINS` query language and the tokenizer it runs
//! over, as the original engine evaluates them.
//!
//! A full-text index tokenizes a field's text into words — lower-cased unless the
//! index is case-sensitive, split on the index's delimiters, trimmed of its
//! conditional characters at the edges, with its drop characters removed
//! throughout, and words shorter than the minimum or in the noise list left out.
//! `CONTAINS(field, condition)` parses `condition` into a boolean tree of words
//! (with `*` wildcards), phrases, `AND`/`OR`/`NOT` and `NEAR`, and matches it
//! against a record's word list. This module is the engine for both; reading an
//! index's own parsing rules, and wiring it into SQL, live elsewhere.
//!
//! The error numbers are the engine's native ones (`CONTAINS` raises them
//! directly): 7094 an unterminated phrase, 7095 an unterminated group, 7096 a
//! missing or misplaced operand, 7097 an unexpected `)`, 7098 a `NOT` inside a
//! `NEAR` group, 7099 a bad `NEAR` distance or group, 7103 a condition with no
//! searchable word.

use std::collections::HashSet;

use tflip_storage::adi::FtsOptions;
use tflip_storage::create::FtsBlock;

/// The engine's default noise (stop) words, the list a dictionary falls back to.
const DEFAULT_NOISE: &str = "about after all also an and another any are as at be because been \
    before being between both but by came can come could did do does each else for from get \
    got had has have he her here him himself his how if in into is it its just like make many \
    me might more most much must my never now of on only or other our out over re said same \
    see should since so some still such take than that the their them then there these they \
    this those through to too under up use very want was way we well were what when where \
    which while who will with would you your";

/// The default conditional characters: kept inside a word, trimmed at its edges.
const DEFAULT_CONDITIONALS: &str = ",.?!;:@#$%^&()-_";
/// The default drop characters: removed from the text and from search words.
const DEFAULT_DROP: &str = "'\"`";

/// The `ADS_FTS_*` option flags that change tokenizing: an index stores them in
/// its options word (see [`tflip_storage::adi::FtsOptions`]).
const FTS_CASE_SENSITIVE: u32 = 0x80;
const FTS_PROTECT_NUMBERS: u32 = 0x200;
/// `ADS_FTS_FIXED`: the order is not kept current as its records change.
pub const FTS_FIXED: u32 = 0x40;

/// How a full-text index tokenizes text. The defaults are the engine's; an index
/// may override them (case, lengths, and the character classes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub delimiters: Vec<char>,
    pub drop_chars: Vec<char>,
    pub conditionals: Vec<char>,
    pub noise: HashSet<String>,
    pub min_word: usize,
    pub max_word: usize,
    pub case_sensitive: bool,
    /// Keep a delimiter that sits between two digits inside the word, so a
    /// number like `1,423.99` is one token (`ADS_FTS_PROTECT_NUMBERS`).
    pub protect_numbers: bool,
    /// The keys of a FIXED (not maintained) order, which answers `CONTAINS`
    /// and `SCOREDISTINCT` from them; `None` for an order kept current (whose
    /// keys are its records' text) and for a field with no order.
    pub postings: Option<std::sync::Arc<Postings>>,
}

impl Rules {
    /// The rules a default full-text index uses.
    pub fn indexed_default() -> Rules {
        Rules {
            delimiters: " \t\n\x0b\x0c\r\x08".chars().collect(),
            drop_chars: DEFAULT_DROP.chars().collect(),
            conditionals: DEFAULT_CONDITIONALS.chars().collect(),
            noise: noise_set(DEFAULT_NOISE),
            min_word: 3,
            max_word: 30,
            case_sensitive: false,
            protect_numbers: false,
            postings: None,
        }
    }

    /// The rules for `CONTAINS` on a field with no full-text index: every word
    /// counts — no noise list and no minimum length — but the text is split and
    /// trimmed the default way.
    pub fn non_indexed() -> Rules {
        Rules {
            noise: HashSet::new(),
            min_word: 1,
            ..Rules::indexed_default()
        }
    }

    /// The rules a full-text order tokenizes with, as read from its `.adi`
    /// (`AdsGetFTSIndexInfo`): its character sets, word bounds and the option
    /// flags that set case folding and number protection.
    pub fn from_options(options: &FtsOptions) -> Rules {
        Rules {
            delimiters: options.delimiters.chars().collect(),
            drop_chars: options.drop.chars().collect(),
            conditionals: options.conditional.chars().collect(),
            noise: noise_set(&options.noise),
            min_word: options.min_word as usize,
            max_word: options.max_word as usize,
            case_sensitive: options.options & FTS_CASE_SENSITIVE != 0,
            protect_numbers: options.options & FTS_PROTECT_NUMBERS != 0,
            postings: None,
        }
    }

    fn fold(&self, s: &str) -> String {
        if self.case_sensitive {
            s.to_owned()
        } else {
            s.to_ascii_lowercase()
        }
    }

    /// Whether the character at `i` in `chars` splits words: a delimiter, but
    /// not one kept between two digits when numbers are protected.
    fn splits(&self, chars: &[char], i: usize) -> bool {
        let c = chars[i];
        if !self.delimiters.contains(&c) {
            return false;
        }
        if self.protect_numbers
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
        {
            return false;
        }
        true
    }
}

/// The default delimiters in the order an index's options block stores them.
const STORED_DELIMITERS: &str = "\x08\t\n\x0b\x0c\r ";

/// `ADS_FTS_KEEP_SCORE`: the index counts each word's occurrences.
const FTS_KEEP_SCORE: u32 = 0x100;

/// What `CREATE INDEX … CONTENT` refuses of its options: the parameter the
/// original engine names, and why (5019, `fixtures/write/fts_build`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadOption {
    pub parameter: u32,
    pub why: &'static str,
}

/// The options block of a full-text index made with `options`, as the
/// original engine stores it (`fixtures/write/fts_build`): words of 3 to 30
/// letters by default; each list of characters the given ones and then the
/// defaults (`NEW` only the given ones), as they come; the noise words as
/// given (SQL gives them in lower case), the given and the default ones sorted
/// together by their bytes, those shorter than the shortest word left out.
pub fn index_block(options: &crate::ast::FullText) -> Result<FtsBlock, BadOption> {
    let min_word = options.min_word.unwrap_or(3).min(u64::from(u32::MAX)) as u32;
    let max_word = options.max_word.unwrap_or(30).min(u64::from(u32::MAX)) as u32;
    let merged = |given: &Option<(bool, String)>, default: &str| match given {
        Some((true, text)) => text.clone(),
        Some((false, text)) => format!("{text}{default}"),
        None => default.to_owned(),
    };
    let delimiters = merged(&options.delimiters, STORED_DELIMITERS);
    if delimiters.is_empty() {
        return Err(BadOption {
            parameter: 9,
            why: "No delimiters provided.  Either use default delimiters or provide a delimiter \
                  list.",
        });
    }
    if min_word > max_word {
        return Err(BadOption {
            parameter: 6,
            why: "Minimum word length must be less than maximum word length.",
        });
    }
    if max_word == 0 {
        return Err(BadOption {
            parameter: 7,
            why: "Maximum word length cannot be 0.",
        });
    }
    let noise = match &options.noise {
        Some((false, given)) => format!("{given} {DEFAULT_NOISE}"),
        _ => merged(&options.noise, DEFAULT_NOISE),
    };
    let mut noise: Vec<String> = noise
        .split_whitespace()
        .filter(|word| word.chars().count() >= min_word as usize)
        .map(str::to_owned)
        .collect();
    noise.sort_by_key(|word| encode(word));
    let flags = [
        (options.fixed, FTS_FIXED),
        (options.case_sensitive, FTS_CASE_SENSITIVE),
        (options.keep_score, FTS_KEEP_SCORE),
        (options.protect_numbers, FTS_PROTECT_NUMBERS),
    ];
    Ok(FtsBlock {
        options: flags
            .iter()
            .filter(|(set, _)| *set)
            .fold(0, |all, (_, bit)| all | bit),
        min_word,
        max_word,
        delimiters: encode(&delimiters),
        noise: encode(&noise.join(" ")),
        drop: encode(&merged(&options.drop, DEFAULT_DROP)),
        conditional: encode(&merged(&options.conditionals, DEFAULT_CONDITIONALS)),
    })
}

/// The noise words a dictionary records for a full-text order made with
/// `options`: as given and then the defaults (`NEW` only the given ones),
/// neither sorted nor cut by the shortest word as the index's own are
/// (`fixtures/aes/dictwrite`, `43_fts_dictionary`).
pub fn dictionary_noise(options: &crate::ast::FullText) -> Vec<u8> {
    match &options.noise {
        Some((false, given)) => encode(&format!("{given} {DEFAULT_NOISE}")),
        Some((true, given)) => encode(given),
        None => encode(DEFAULT_NOISE),
    }
}

fn encode(text: &str) -> Vec<u8> {
    tflip_storage::cp1252::encode(text)
}

/// The noise list as a set, each word folded to lower case.
pub fn noise_set(list: &str) -> HashSet<String> {
    list.split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

/// Tokenizes `text` into the word list the index would hold for it: drop
/// characters removed, split on delimiters, conditional characters trimmed at the
/// edges, each word folded, and words shorter than the minimum or in the noise
/// list dropped. Positions in the returned list are what `NEAR` and phrases
/// count. (The engine does not truncate or drop words longer than the maximum,
/// as `fixtures/sql/fts` shows: a 10-letter word is found under a 5-letter
/// maximum.)
pub fn tokenize(text: &str, rules: &Rules) -> Vec<String> {
    let dropped: Vec<char> = text
        .chars()
        .filter(|c| !rules.drop_chars.contains(c))
        .collect();
    let mut words = Vec::new();
    let mut start = 0;
    for i in 0..=dropped.len() {
        if i == dropped.len() || rules.splits(&dropped, i) {
            if start < i {
                push_word(
                    &dropped[start..i].iter().collect::<String>(),
                    rules,
                    &mut words,
                );
            }
            start = i + 1;
        }
    }
    words
}

/// Trims, folds, bounds and filters a split-off run, adding it to `words` if it
/// survives (not empty, not noise, long enough).
fn push_word(raw: &str, rules: &Rules, words: &mut Vec<String>) {
    let trimmed = raw.trim_matches(|c| rules.conditionals.contains(&c));
    if trimmed.is_empty() {
        return;
    }
    let word = rules.fold(trimmed);
    if word.chars().count() < rules.min_word || rules.noise.contains(&word) {
        return;
    }
    words.push(word);
}

/// A parsed `CONTAINS` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// A word, possibly with a leading and/or trailing `*`.
    Word {
        body: String,
        prefix: bool,
        suffix: bool,
    },
    /// A phrase: consecutive words.
    Phrase(Vec<String>),
    And(Box<Query>, Box<Query>),
    Or(Box<Query>, Box<Query>),
    Not(Box<Query>),
    Near(u64, Box<Query>, Box<Query>),
}

/// A `CONTAINS` parse or evaluation failure, carrying the engine's native error
/// number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtsError(pub u32);

impl FtsError {
    /// The original engine's text for the error (`AdsGetLastError`).
    pub fn text(self) -> &'static str {
        match self.0 {
            7094 => {
                "The full text search condition has an unclosed phrase. Verify that quoted words \
                 and phrases have closing quotes."
            }
            7095 => {
                "The full text search condition has an unclosed expression. Verify that the \
                 parentheses are balanced."
            }
            7096 => {
                "The full text search condition is missing a phrase or word. Verify that the \
                 condition is not empty and that the logical operators are balanced with search \
                 words."
            }
            7097 => {
                "The full text search condition is not valid. Verify that the parentheses and \
                 logical operators are balanced."
            }
            7098 => {
                "The full text search condition has invalid operands specified for the NEAR \
                 operator. Verify that the operands to the NEAR operator are words or phrases."
            }
            7099 => {
                "The full text search condition has an invalid distance specified for the NEAR \
                 operator. Verify that the NEAR parameter value is numeric."
            }
            7102 => {
                "The table does not have any full text search indexes.  Use of CONTAINS( *, \
                 'condition' ) requires at least one full text search index."
            }
            7103 => {
                "The full text search condition does not have any valid search words.  Verify \
                 that it consists of at least one non-noise word."
            }
            _ => "Full-text search error.",
        }
    }
}

const DEFAULT_NEAR: u64 = 8;

/// Parses a `CONTAINS` condition into a [`Query`], tokenizing its words with
/// `rules`. The noise and minimum-length rules drop words from the condition too
/// (a wildcard word is kept regardless); a condition left with no word is 7103.
pub fn parse(condition: &str, rules: &Rules) -> Result<Query, FtsError> {
    let lexemes = lex(condition, rules)?;
    if lexemes.is_empty() {
        // Nothing but delimiters is 7096; a truly empty string is 7103.
        return Err(FtsError(
            if condition.trim().is_empty() && !condition.is_empty() {
                7096
            } else {
                7103
            },
        ));
    }
    let mut parser = Parser {
        lexemes: &lexemes,
        at: 0,
    };
    let query = parser.expr(Mode::Normal)?;
    if parser.at != parser.lexemes.len() {
        // A leftover `)` is 7097; anything else a misplaced operand.
        return Err(FtsError(match parser.peek() {
            Some(Lexeme::Close) => 7097,
            _ => 7096,
        }));
    }
    // Nothing searchable left once the dropped words are gone is 7103.
    query.ok_or(FtsError(7103))
}

/// `a op b` of two operands of which a dropped word (`None`) leaves the other
/// standing alone, as the original engine reads `medical near x` as `medical`.
fn joined(
    a: Option<Query>,
    b: Option<Query>,
    op: impl FnOnce(Box<Query>, Box<Query>) -> Query,
) -> Option<Query> {
    match (a, b) {
        (Some(a), Some(b)) => Some(op(Box::new(a), Box::new(b))),
        (a, None) => a,
        (None, b) => b,
    }
}

/// A lexeme of a `CONTAINS` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lexeme {
    Term(Option<Query>), // a Word or Phrase; `None` for a word dropped
    And,
    Or,
    Not,
    Near(u64, bool), // distance, explicit (n) given
    Open,
    Close,
}

/// Splits a condition into lexemes, folding and dropping words as `rules` say.
/// A word that becomes noise or too short is left out (so `the medical` lexes to
/// just the word `medical`); a wildcard word is always kept.
fn lex(condition: &str, rules: &Rules) -> Result<Vec<Lexeme>, FtsError> {
    let chars: Vec<char> = condition.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' || rules.splits(&chars, i) {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                out.push(Lexeme::Open);
                i += 1;
            }
            ')' => {
                out.push(Lexeme::Close);
                i += 1;
            }
            '"' => {
                // A phrase: up to the next quote (7094 if none).
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '"' {
                    j += 1;
                }
                if j >= chars.len() {
                    return Err(FtsError(7094));
                }
                let phrase: String = chars[i + 1..j].iter().collect();
                let words = tokenize(&phrase, rules);
                out.push(Lexeme::Term(
                    (!words.is_empty()).then_some(Query::Phrase(words)),
                ));
                i = j + 1;
            }
            _ => {
                // A run up to the next delimiter, space, paren or quote.
                let mut j = i;
                while j < chars.len() {
                    let d = chars[j];
                    if d == ' ' || d == '(' || d == ')' || d == '"' || rules.splits(&chars, j) {
                        break;
                    }
                    j += 1;
                }
                let run: String = chars[i..j].iter().collect();
                let lower = run.to_ascii_lowercase();
                match lower.as_str() {
                    "and" => out.push(Lexeme::And),
                    "or" => out.push(Lexeme::Or),
                    "not" => out.push(Lexeme::Not),
                    "near" => {
                        // `near` may be followed immediately by `(distance)`.
                        if j < chars.len() && chars[j] == '(' {
                            let mut k = j + 1;
                            while k < chars.len() && chars[k] != ')' {
                                k += 1;
                            }
                            if k >= chars.len() {
                                return Err(FtsError(7095));
                            }
                            let inside: String = chars[j + 1..k].iter().collect();
                            let dist = near_distance(&inside)?;
                            out.push(Lexeme::Near(dist, true));
                            i = k + 1;
                            continue;
                        }
                        out.push(Lexeme::Near(DEFAULT_NEAR, false));
                    }
                    // A noise or too-short word is an operand that drops
                    // out, with the operator that joins it.
                    _ => out.push(Lexeme::Term(search_word(&run, rules))),
                }
                i = j;
            }
        }
    }
    Ok(out)
}

/// The distance in `near(...)`: the leading integer (`strtol`, trailing junk
/// ignored); empty is the default; zero or negative is 7099.
fn near_distance(inside: &str) -> Result<u64, FtsError> {
    let s = inside.trim();
    if s.is_empty() {
        return Ok(DEFAULT_NEAR);
    }
    let negative = s.starts_with('-');
    let digits: String = s
        .trim_start_matches(['-', '+'])
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return Err(FtsError(7099));
    }
    let value: u64 = digits.parse().unwrap_or(u64::MAX);
    if negative || value == 0 {
        return Err(FtsError(7099));
    }
    Ok(value)
}

/// A search word from the condition: trimmed of conditional characters, drop
/// characters removed, folded. `*` at the start or end is a wildcard; it keeps
/// the word even when it would be noise or too short. Returns `None` if nothing
/// searchable is left (a plain noise or too-short word).
fn search_word(run: &str, rules: &Rules) -> Option<Query> {
    let no_drop: String = run
        .chars()
        .filter(|c| !rules.drop_chars.contains(c))
        .collect();
    let prefix = no_drop.starts_with('*');
    let suffix = no_drop.ends_with('*');
    let core = no_drop.trim_matches('*');
    let core = core.trim_matches(|c: char| rules.conditionals.contains(&c));
    if core.is_empty() {
        return None;
    }
    let body = rules.fold(core);
    if prefix || suffix {
        // Wildcards skip the minimum-length and noise rules.
        return Some(Query::Word {
            body,
            prefix,
            suffix,
        });
    }
    if body.chars().count() < rules.min_word || rules.noise.contains(&body) {
        return None;
    }
    Some(Query::Word {
        body,
        prefix: false,
        suffix: false,
    })
}

/// The parsing context: inside a parenthesized group that is the operand of a
/// `NEAR`, a `NOT` is error 7098 rather than allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    NearGroup,
}

/// A recursive-descent parser over the lexemes. Precedence, tightest first:
/// `NEAR`, then `NOT`, then `AND` (and juxtaposition), then `OR`.
struct Parser<'a> {
    lexemes: &'a [Lexeme],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Lexeme> {
        self.lexemes.get(self.at)
    }

    fn bump(&mut self) -> Option<&Lexeme> {
        let l = self.lexemes.get(self.at);
        if l.is_some() {
            self.at += 1;
        }
        l
    }

    /// An OR of ANDs; `None` where every operand dropped out.
    fn expr(&mut self, mode: Mode) -> Result<Option<Query>, FtsError> {
        let mut left = self.and_expr(mode)?;
        while matches!(self.peek(), Some(Lexeme::Or)) {
            self.bump();
            let right = self.and_expr(mode)?;
            left = joined(left, right, Query::Or);
        }
        Ok(left)
    }

    fn and_expr(&mut self, mode: Mode) -> Result<Option<Query>, FtsError> {
        let mut left = self.not_expr(mode)?;
        loop {
            match self.peek() {
                Some(Lexeme::And) => {
                    self.bump();
                    left = joined(left, self.not_expr(mode)?, Query::And);
                }
                // Juxtaposition is an implied AND.
                Some(Lexeme::Term(_) | Lexeme::Open | Lexeme::Not) => {
                    left = joined(left, self.not_expr(mode)?, Query::And);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// `NOT` binds looser than `NEAR`, so it wraps a whole `NEAR` expression.
    fn not_expr(&mut self, mode: Mode) -> Result<Option<Query>, FtsError> {
        if matches!(self.peek(), Some(Lexeme::Not)) {
            if mode == Mode::NearGroup {
                return Err(FtsError(7098));
            }
            self.bump();
            if matches!(self.peek(), Some(Lexeme::Not)) {
                return Err(FtsError(7096)); // NOT NOT
            }
            return Ok(self
                .not_expr(mode)?
                .map(|inner| Query::Not(Box::new(inner))));
        }
        self.near_expr(mode)
    }

    /// `NEAR` is right-associative (`a near b near c` is `a near (b near c)`), and
    /// its right operand is a term or group in a `NEAR` context: a bare `NOT` there
    /// is 7096, a `NOT` inside the group 7098.
    fn near_expr(&mut self, mode: Mode) -> Result<Option<Query>, FtsError> {
        let left = self.primary(mode)?;
        if let Some(Lexeme::Near(dist, explicit)) = self.peek() {
            let (dist, explicit) = (*dist, *explicit);
            self.bump();
            // A bare NEAR (no explicit distance) may not take a parenthesized group.
            if !explicit && matches!(self.peek(), Some(Lexeme::Open)) {
                return Err(FtsError(7099));
            }
            let right = self.near_expr(Mode::NearGroup)?;
            return Ok(joined(left, right, |a, b| Query::Near(dist, a, b)));
        }
        Ok(left)
    }

    fn primary(&mut self, mode: Mode) -> Result<Option<Query>, FtsError> {
        match self.peek() {
            Some(Lexeme::Term(_)) => {
                let Some(Lexeme::Term(q)) = self.bump() else {
                    unreachable!()
                };
                Ok(q.clone())
            }
            Some(Lexeme::Open) => {
                self.bump();
                let inner = self.expr(mode)?;
                match self.bump() {
                    Some(Lexeme::Close) => Ok(inner),
                    _ => Err(FtsError(7095)),
                }
            }
            // An operator (NOT, NEAR, AND, OR) or the end where a term was wanted.
            _ => Err(FtsError(7096)),
        }
    }
}

/// Whether the index fully optimizes a query: only words, no phrase and no
/// `NEAR`. A fully-optimized query is answered from the index alone, so it also
/// matches rows the index keeps that have no words (a NULL field); one that is
/// not drops those rows.
pub fn fully_optimized(query: &Query) -> bool {
    match query {
        Query::Word { .. } => true,
        Query::Phrase(_) | Query::Near(..) => false,
        Query::Not(a) => fully_optimized(a),
        Query::And(a, b) | Query::Or(a, b) => fully_optimized(a) && fully_optimized(b),
    }
}

/// How far a full-text index answers a query, as the original engine's plans
/// report it (`fixtures/sql/fts_plans.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexLevel {
    /// From its keys alone.
    Full,
    /// Its keys narrow the records, which are then read: a phrase or `NEAR`
    /// (the keys hold no positions), or a word as long as the longest word the
    /// index keeps (its keys are cut to that length).
    Partial,
    /// Not at all: a `NOT` of a part the keys only narrow.
    None,
}

/// How far a full-text index whose longest word is `max_word` answers `query`.
pub fn index_level(query: &Query, max_word: usize) -> IndexLevel {
    match query {
        Query::Word { body, .. } if body.chars().count() >= max_word => IndexLevel::Partial,
        Query::Word { .. } => IndexLevel::Full,
        Query::Phrase(_) => IndexLevel::Partial,
        Query::Near(_, a, b) => index_level(a, max_word)
            .max(index_level(b, max_word))
            .max(IndexLevel::Partial),
        Query::And(a, b) | Query::Or(a, b) => {
            index_level(a, max_word).max(index_level(b, max_word))
        }
        Query::Not(a) => match index_level(a, max_word) {
            IndexLevel::Full => IndexLevel::Full,
            _ => IndexLevel::None,
        },
    }
}

/// A full-text order's keys: the distinct words of each record, as the index
/// holds them (folded unless case-sensitive, cut to the longest word it keeps).
/// A FIXED (not maintained) order is answered from them rather than from the
/// records' text, which may have changed since it was built; and a plan counts
/// the records an order's keys find.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Postings {
    /// Each record's words, by record number.
    words: std::collections::BTreeMap<u32, Vec<String>>,
    /// The longest word the order keeps: its key length.
    max_word: usize,
}

impl Postings {
    /// The postings of an order whose keys are `keys` (each a word, padded with
    /// blanks, and its record) and `max_word` long.
    pub fn new(keys: impl IntoIterator<Item = (String, u32)>, max_word: usize) -> Postings {
        let mut words: std::collections::BTreeMap<u32, Vec<String>> = Default::default();
        for (word, record) in keys {
            words
                .entry(record)
                .or_default()
                .push(word.trim_end_matches(' ').to_owned());
        }
        Postings { words, max_word }
    }

    /// The words the order holds for `record` (none if it has no key).
    pub fn of(&self, record: u32) -> &[String] {
        self.words.get(&record).map_or(&[], Vec::as_slice)
    }

    /// Whether `record`'s keys hold the query word `body` with its wildcards.
    fn has(&self, record: u32, body: &str, prefix: bool, suffix: bool) -> bool {
        keys_have(self.of(record), self.max_word, body, prefix, suffix)
    }

    /// Whether `record`'s keys hold `query` (see [`keys_hold`]).
    pub fn holds(&self, record: u32, query: &Query) -> bool {
        keys_hold(self.of(record), self.max_word, query)
    }

    /// The records among `1..=records` (a table's record count, deleted
    /// records included) whose keys hold `query`: those the order finds for it.
    pub fn found(&self, query: &Query, records: u32) -> Vec<u32> {
        (1..=records).filter(|&r| self.holds(r, query)).collect()
    }

    /// `SCOREDISTINCT` of `query` over `record`, as a FIXED order answers it:
    /// how many of the query's words (outside a `NOT`, a phrase's each) its keys
    /// hold; a word as long as the key length is found only if the record's
    /// `words` (its text, tokenized) still hold it whole.
    pub fn score_distinct(&self, record: u32, query: &Query, words: &[String]) -> u64 {
        let found = |body: &str, prefix: bool, suffix: bool| {
            self.has(record, body, prefix, suffix)
                && (body.chars().count() < self.max_word
                    || words.iter().any(|w| word_matches(w, body, prefix, suffix)))
        };
        let mut total = 0;
        each_word(query, &mut |body, prefix, suffix| {
            total += u64::from(found(body, prefix, suffix))
        });
        total
    }
}

/// The keys an order whose longest word is `max_word` keeps for a record whose
/// text has the words `words`: each cut to that length.
pub fn keys_of(words: &[String], max_word: usize) -> Vec<String> {
    let mut keys: Vec<String> = words
        .iter()
        .map(|w| w.chars().take(max_word).collect())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Whether a record's full-text keys `keys` (words cut to `max_word`) hold the
/// query word `body` with its wildcards: the word cut to the key length too.
fn keys_have(keys: &[String], max_word: usize, body: &str, prefix: bool, suffix: bool) -> bool {
    let cut: String = body.chars().take(max_word).collect();
    keys.iter().any(|w| word_matches(w, &cut, prefix, suffix))
}

/// Whether a record's full-text keys `keys` (words cut to `max_word`) hold
/// `query`, as its order finds the record: a phrase or `NEAR` holds when its
/// words all do (the keys keep no positions); `NOT` is the lack of the part.
pub fn keys_hold(keys: &[String], max_word: usize, query: &Query) -> bool {
    let has = |body: &str, prefix, suffix| keys_have(keys, max_word, body, prefix, suffix);
    match query {
        Query::Word {
            body,
            prefix,
            suffix,
        } => has(body, *prefix, *suffix),
        Query::Phrase(words) => !words.is_empty() && words.iter().all(|w| has(w, false, false)),
        Query::And(a, b) | Query::Near(_, a, b) => {
            keys_hold(keys, max_word, a) && keys_hold(keys, max_word, b)
        }
        Query::Or(a, b) => keys_hold(keys, max_word, a) || keys_hold(keys, max_word, b),
        Query::Not(a) => !keys_hold(keys, max_word, a),
    }
}

/// Calls `f` on each word of `query` outside a `NOT` (a phrase's each), with its
/// wildcards.
fn each_word(query: &Query, f: &mut dyn FnMut(&str, bool, bool)) {
    match query {
        Query::Word {
            body,
            prefix,
            suffix,
        } => f(body, *prefix, *suffix),
        Query::Phrase(words) => words.iter().for_each(|w| f(w, false, false)),
        Query::And(a, b) | Query::Or(a, b) | Query::Near(_, a, b) => {
            each_word(a, f);
            each_word(b, f);
        }
        Query::Not(_) => {}
    }
}

/// Matches a parsed query against a record's word list.
pub fn matches(query: &Query, words: &[String]) -> bool {
    match query {
        Query::Not(inner) => !matches(inner, words),
        _ => !ranges(query, words).is_empty(),
    }
}

/// The position ranges where `query` matches `words`: for a word, each matching
/// position as `(p, p)`; for a phrase, the span of the match; for `AND`/`NEAR`,
/// the combined span of a left and a right match (`NEAR` keeping only spans no
/// wider than its distance); for `OR`, the union. `NOT` yields no ranges (it is
/// a pure boolean, handled by [`matches`]); inside another operator a `NOT` that
/// holds contributes the whole record as a span.
fn ranges(query: &Query, words: &[String]) -> Vec<(usize, usize)> {
    match query {
        Query::Word {
            body,
            prefix,
            suffix,
        } => words
            .iter()
            .enumerate()
            .filter(|(_, w)| word_matches(w, body, *prefix, *suffix))
            .map(|(i, _)| (i, i))
            .collect(),
        Query::Phrase(phrase) => phrase_ranges(phrase, words),
        Query::Or(a, b) => {
            let mut r = ranges(a, words);
            r.extend(ranges(b, words));
            r
        }
        Query::And(a, b) => combine(&ranges(a, words), &ranges(b, words), a, b, words, None),
        Query::Near(dist, a, b) => combine(
            &ranges(a, words),
            &ranges(b, words),
            a,
            b,
            words,
            Some(*dist),
        ),
        Query::Not(inner) => {
            // Within an enclosing span operator, a satisfied NOT spans nothing in
            // particular; treat it as the whole record so AND keeps the other side.
            if matches(inner, words) || words.is_empty() {
                Vec::new()
            } else {
                vec![(0, words.len() - 1)]
            }
        }
    }
}

/// Combines left and right ranges into their enclosing spans, keeping only those
/// no wider than `within` (for `NEAR`). A `NOT` operand that holds lets the other
/// side stand (an `AND` with a `NOT`).
fn combine(
    left: &[(usize, usize)],
    right: &[(usize, usize)],
    a: &Query,
    b: &Query,
    words: &[String],
    within: Option<u64>,
) -> Vec<(usize, usize)> {
    if within.is_none() {
        if matches!(a, Query::Not(_)) {
            return if matches(a, words) {
                right.to_vec()
            } else {
                Vec::new()
            };
        }
        if matches!(b, Query::Not(_)) {
            return if matches(b, words) {
                left.to_vec()
            } else {
                Vec::new()
            };
        }
    }
    let mut out = Vec::new();
    for &(l0, l1) in left {
        for &(r0, r1) in right {
            // NEAR needs two distinct matches, not one word counted twice.
            if within.is_some() && (l0, l1) == (r0, r1) {
                continue;
            }
            let lo = l0.min(r0);
            let hi = l1.max(r1);
            if let Some(n) = within
                && (hi - lo) as u64 > n
            {
                continue;
            }
            out.push((lo, hi));
        }
    }
    out
}

/// The spans where `phrase` appears as consecutive words in `words`.
fn phrase_ranges(phrase: &[String], words: &[String]) -> Vec<(usize, usize)> {
    if phrase.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if phrase.len() > words.len() {
        return out;
    }
    for start in 0..=words.len() - phrase.len() {
        if phrase.iter().zip(&words[start..]).all(|(p, w)| p == w) {
            // For NEAR a phrase counts as a single point at its last word.
            let end = start + phrase.len() - 1;
            out.push((end, end));
        }
    }
    out
}

/// Whether record word `word` matches the search `body` with the given
/// wildcards: no wildcard is an exact match, `prefix` a trailing match, `suffix`
/// a leading match, both a substring.
fn word_matches(word: &str, body: &str, prefix: bool, suffix: bool) -> bool {
    match (prefix, suffix) {
        (false, false) => word == body,
        (false, true) => word.starts_with(body),
        (true, false) => word.ends_with(body),
        (true, true) => word.contains(body),
    }
}

/// Runs a `CONTAINS` condition over `text`, tokenizing both with `rules`.
pub fn search(condition: &str, text: &str, rules: &Rules) -> Result<bool, FtsError> {
    let query = parse(condition, rules)?;
    Ok(matches(&query, &tokenize(text, rules)))
}

/// `SCORE`/`SCOREDISTINCT` of a parsed query over a tokenized field. `SCORE`
/// (`distinct` false) sums, over every word of the query, how many of the
/// field's words it matches; `SCOREDISTINCT` counts the query's words that
/// match at least once. A phrase counts as its words and `NEAR` as its sides,
/// whether or not they stand together (`"doctors medical"` scores where the text
/// has `medical doctors`); words under a `NOT` count nothing (so `NOT x` scores
/// 0).
pub fn score(query: &Query, words: &[String], distinct: bool) -> u64 {
    let mut acc = 0;
    each_word(query, &mut |body, prefix, suffix| {
        let n = words
            .iter()
            .filter(|w| word_matches(w, body, prefix, suffix))
            .count() as u64;
        acc += if distinct { u64::from(n > 0) } else { n };
    });
    acc
}
