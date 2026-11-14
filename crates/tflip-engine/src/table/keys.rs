//! Keys of a table's records in its index orders: built from records or
//! values for ADI, NTX and CDX orders, the keys seeks look for, and the
//! keys clients add to custom orders.

use super::*;
use tflip_storage::key::sortable;

impl Table {
    /// The key `value` seeks in `order` (see [`seek::seek_key`]), weighed as
    /// the order's keys are (see [`Table::weigh_sought`]). An NTX order's
    /// keys are text, its numbers and dates too (see [`seek::ntx_seek_key`]),
    /// and its cursor weighs them.
    pub(super) fn sought_key(&self, order: &Index, value: SeekValue) -> Result<Vec<u8>, Error> {
        let raw = matches!(value, SeekValue::Raw(_));
        let source = order.key_source(self);
        if order.kind == tflip_storage::order::Kind::Cdx {
            let expression = match source {
                seek::KeySource::Expression => seek::CdxExpression {
                    stamp: self
                        .compile(&order.expression())
                        .is_ok_and(|expr| expr.ty() == tflip_expr::Type::TimeStamp),
                    nullable: order.tag.flags & tflip_storage::order::flags::NULLABLE != 0,
                    numeric: order.key_type() == index::KeyType::Numeric
                        && !matches!(value, SeekValue::Text(text) if seek::number_text(text).is_none()),
                },
                _ => seek::CdxExpression::default(),
            };
            return seek::cdx_seek_key(
                value,
                source,
                usize::from(order.tag.key_length),
                expression,
                &self.date_format(),
                self.session_settings.epoch(),
            );
        }
        if order.kind == tflip_storage::order::Kind::Ntx
            && matches!(source, seek::KeySource::Expression)
            && !raw
        {
            // An expression order's keys are text: a number seeks a numeric
            // one as its key, a double a character one as its bytes (finding
            // nothing), and a logical one not at all
            // (`fixtures/expr/numkeys.json`, `fixtures/expr/alias.json`).
            let length = usize::from(order.tag.key_length);
            let number = match value {
                SeekValue::Double(value) => Some(value),
                SeekValue::Text(text) if order.key_type() == index::KeyType::Numeric => {
                    seek::number_text(text)
                }
                _ => None,
            };
            match (order.key_type(), number) {
                (index::KeyType::Numeric, Some(value)) => {
                    let decimals = usize::from(order.tag.decimals);
                    let mut key = tflip_storage::ntx::number_key(value, length, decimals);
                    key.truncate(length);
                    return Ok(key);
                }
                (index::KeyType::Logical, Some(_)) => return Err(Error::InvalidExpression),
                (index::KeyType::String, Some(value)) => {
                    let mut key = value.to_le_bytes().to_vec();
                    key.truncate(length);
                    return Ok(key);
                }
                _ => {}
            }
        }
        if order.kind == tflip_storage::order::Kind::Ntx {
            return seek::ntx_seek_key(
                value,
                source,
                usize::from(order.tag.key_length),
                order.tag.decimals,
                &self.date_format(),
                self.session_settings.epoch(),
            );
        }
        // An order on an expression of one Numeric field (`ID + 1`, `ID * 2`)
        // is sought as the original seeks it: the number put in that field,
        // rounded to its decimals, and the key of that record's value; the
        // others are left blank (`fixtures/expr/numkeys.json`).
        if !raw
            && matches!(source, seek::KeySource::Expression)
            && order.key_type() == index::KeyType::Numeric
            && let Some(key) = self.numeric_field_key(order, value)?
        {
            return Ok(key);
        }
        // A numeric or date expression order keys a double seek value as a
        // double — the same key `build_key` stores — not the raw bytes
        // `seek_key` gives a character expression (probed, `fixtures/expr`).
        if let (SeekValue::Double(value), seek::KeySource::Expression) = (value, &source)
            && matches!(
                order.key_type(),
                index::KeyType::Numeric | index::KeyType::Date
            )
        {
            return Ok(tflip_storage::key::double(value).to_vec());
        }
        // A number given as text seeks a numeric expression order as the
        // number `atof` reads (`fixtures/expr/numkeys.json`).
        if let (SeekValue::Text(text), seek::KeySource::Expression) = (value, &source)
            && order.key_type() == index::KeyType::Numeric
            && let Some(value) = seek::number_text(text)
        {
            return Ok(tflip_storage::key::double(value).to_vec());
        }
        // An expression of Unicode text is sought as a Unicode field is: by
        // the sort key of the whole text (`fixtures/write/nchar_expr`).
        if matches!(source, seek::KeySource::Expression)
            && !raw
            && let Ok(expression) = self.compile(&order.expression())
            && matches!(
                expression.unicode(),
                tflip_expr::Unicode::Text { .. } | tflip_expr::Unicode::Narrow { .. }
            )
        {
            let SeekValue::Text(text) = value else {
                return Err(Error::NotImplemented);
            };
            let mut key = tflip_storage::icu::sort_key(tflip_storage::icu::nchar_text(text))
                .ok_or(Error::NotImplemented)?;
            key.truncate(usize::from(order.tag.key_length));
            return Ok(key);
        }
        let key = seek::seek_key(
            value,
            source,
            usize::from(order.tag.key_length),
            &self.date_format(),
            self.session_settings.epoch(),
        )?;
        Ok(if raw {
            key
        } else {
            self.weigh_sought(order, key)
        })
    }

    /// The key an ADT order on an expression of one Numeric field gives a
    /// record whose field holds `value` (a number given as text read as
    /// `atof` reads it), the field's value rounded half away from zero to
    /// its decimals and every other field blank; `None` if the expression
    /// reads another field, or none, or the value does not fit.
    fn numeric_field_key(&self, order: &Index, value: SeekValue) -> Result<Option<Vec<u8>>, Error> {
        let number = match value {
            SeekValue::Double(value) => value,
            SeekValue::Text(text) => match seek::number_text(text) {
                Some(number) => number,
                None => return Ok(None),
            },
            SeekValue::Raw(_) => return Ok(None),
        };
        let fields = index::expression_fields(self, &order.expression()).unwrap_or_default();
        let [only] = fields.as_slice() else {
            return Ok(None);
        };
        let field = &self.fields()[*only];
        if field.field_type != FieldType::Numeric {
            return Ok(None);
        }
        let decimals = field.decimals();
        let Some(scaled) = seek::scaled_double(number, decimals) else {
            return Ok(None);
        };
        let digits = scaled.unsigned_abs().to_string();
        let digits = format!("{digits:0>width$}", width = usize::from(decimals) + 1);
        let (whole, fraction) = digits.split_at(digits.len() - usize::from(decimals));
        let sign = if scaled < 0 { "-" } else { "" };
        let text = match decimals {
            0 => format!("{sign}{whole}"),
            _ => format!("{sign}{whole}.{fraction}"),
        };
        if text.len() > usize::from(field.length()) {
            return Ok(None);
        }
        let text = format!("{text:>width$}", width = usize::from(field.length()));
        let mut bytes = self.null_record();
        field
            .encode(&tflip_storage::value::Value::Numeric(text), &mut bytes)
            .map_err(|_| Error::ValueOverflow)?;
        let row = filter::Record {
            fields: self.fields(),
            bytes,
            memo: None,
            number: 0,
        };
        self.build_key(order, row, false)
    }

    /// Whether the text `order` is sought by is Unicode: the first of its
    /// fields is NChar or NVarChar, or its expression is Unicode text.
    pub(super) fn unicode_keys(&self, order: &Index) -> bool {
        match order.key_source(self) {
            seek::KeySource::Field(field) | seek::KeySource::Fields(field) => is_unicode(field),
            seek::KeySource::Expression => {
                self.compile(&order.expression()).is_ok_and(|expression| {
                    matches!(
                        expression.unicode(),
                        tflip_expr::Unicode::Text { .. } | tflip_expr::Unicode::Narrow { .. }
                    )
                })
            }
        }
    }

    /// A sought key as the static ANSI collation weighs it, in an order of
    /// 8-bit text without a dynamic collation (where, as in the original
    /// engine, a sought text is compared as its bytes).
    pub(super) fn weigh_sought(&self, order: &Index, key: Vec<u8>) -> Vec<u8> {
        let collations = self.session_settings.collations();
        let text = match order.key_source(self) {
            seek::KeySource::Field(field) => is_text(field),
            seek::KeySource::Expression => order.key_type() == index::KeyType::String,
            seek::KeySource::Fields(_) => false,
        };
        match collations.ansi() {
            Some(ansi) if text && order.tag.collation.is_empty() => ansi.key(&key),
            _ => key,
        }
    }

    /// The key of record `record` in `order`, built from the record's values
    /// as the original engine builds keys: the field's key for an order on
    /// one field, the fields' keys one after the other for several (`a;b`),
    /// and an expression's value as a key (text padded to the key length,
    /// numbers and dates as doubles, NULL as zeros); `None` if the order's
    /// condition leaves the record out. Orders whose keys are a collation's
    /// weights (a dynamic collation, NChar and NVarChar), and NTX orders,
    /// are [`Error::NotImplemented`].
    pub fn record_key(&self, order: &Index, record: u32) -> Result<Option<Vec<u8>>, Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let bytes = state.read(record, length)?;
        state.with_memo(|memo| {
            self.build_key(
                order,
                filter::Record {
                    fields: self.fields(),
                    bytes,
                    memo,
                    number: record,
                },
                true,
            )
        })
    }

    /// The keys of record `record` in the full-text order `tag`, which
    /// tokenizes with `rules` (see [`words`]).
    pub(crate) fn record_words(
        &self,
        tag: &tflip_storage::order::Tag,
        rules: &tflip_sql::fts::Rules,
        record: u32,
    ) -> Result<Vec<(Vec<u8>, u16)>, Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let bytes = state.read(record, length)?;
        state.with_memo(|memo| {
            let mut row = filter::Record {
                fields: self.fields(),
                bytes,
                memo,
                number: record,
            };
            words(tag, rules, &mut row)
        })
    }

    /// The key of the current record in `order` (`AdsExtractKey`), whatever
    /// the order's condition, as the original engine builds it; at BOF or
    /// EOF, the key of an empty record. See [`Table::record_key`].
    pub fn current_key(&self, order: &Index) -> Result<Vec<u8>, Error> {
        let position = self.state().position;
        let record = position.record;
        // Off a record, the original engine gives an NTX or CDX order's last
        // key at EOF and its first at BOF (probed on NTX orders); a key
        // tflip does not build is the stored one.
        if order.kind != tflip_storage::order::Kind::Adi {
            let key = match record {
                Some(record) => match self.built_key(order, record) {
                    Err(Error::NotImplemented | Error::KeyOverflow) if order.locate(record)? => {
                        order.current_key()?
                    }
                    Err(Error::NotImplemented | Error::KeyOverflow) => None,
                    key => key?,
                },
                None => order.end_key(position.eof)?,
            };
            return key.ok_or(Error::NotImplemented);
        }
        let key = match record {
            Some(record) => self.built_key(order, record)?,
            None => {
                let row = filter::Record {
                    fields: self.fields(),
                    bytes: self.null_record(),
                    memo: None,
                    number: 0,
                };
                self.build_key(order, row, false)?
            }
        };
        key.ok_or(Error::NotImplemented)
    }

    /// The key a bookmark taken through `order` holds: that of the current
    /// record; off a record, that of the record the original engine has in
    /// its buffer there: the table's last record at EOF, and at BOF the
    /// first record of the order, which is where a walk through the order
    /// started (`fixtures/bookmarks/bookmarks.json`; a walk through the
    /// table in record order leaves its first record in the buffer, which
    /// tflip does not follow).
    pub fn bookmark_key(&self, order: &Index) -> Result<Vec<u8>, Error> {
        let position = self.state().position;
        if position.record.is_none() && order.kind == tflip_storage::order::Kind::Adi {
            let last = self.structure.header.record_slots;
            let key = if position.eof && last > 0 {
                self.built_key(order, last)?
            } else {
                order.end_key(false)?
            };
            if let Some(key) = key {
                return Ok(key);
            }
        }
        self.current_key(order)
    }

    /// The key of record `record` in `order`, whatever the order's
    /// condition.
    pub(super) fn built_key(&self, order: &Index, record: u32) -> Result<Option<Vec<u8>>, Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let bytes = state.read(record, length)?;
        state.with_memo(|memo| {
            let row = filter::Record {
                fields: self.fields(),
                bytes,
                memo,
                number: record,
            };
            self.build_key(order, row, false)
        })
    }

    /// The key `order`, whose key is the field `field` alone, has for a
    /// record whose `field` holds `value`; `None` if the field cannot hold
    /// it.
    pub(crate) fn value_key(
        &self,
        order: &Index,
        field: usize,
        value: &Value,
    ) -> Result<Option<Vec<u8>>, Error> {
        let mut bytes = self.null_record.clone();
        let Some(slot) = self.fields().get(field) else {
            return Ok(None);
        };
        if slot.encode(value, &mut bytes).is_err() {
            return Ok(None);
        }
        let row = filter::Record {
            fields: self.fields(),
            bytes,
            memo: None,
            number: 0,
        };
        self.build_key(order, row, false)
    }

    /// The key of the record `row` in `order` (see [`Table::record_key`]);
    /// with `condition`, `None` if the order's condition leaves it out.
    pub(super) fn build_key(
        &self,
        order: &Index,
        row: filter::Record,
        condition: bool,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.build_key_given(order, row, condition, None)
    }

    /// The key in `order` of the value `given` of its expression, as a
    /// record whose expression evaluates to it has; `None` for an order on
    /// fields.
    pub(super) fn expression_value_key(
        &self,
        order: &Index,
        given: tflip_expr::Val,
    ) -> Result<Option<Vec<u8>>, Error> {
        if !matches!(order.key_source(self), seek::KeySource::Expression) {
            return Ok(None);
        }
        let row = filter::Record {
            fields: self.fields(),
            bytes: self.null_record.clone(),
            memo: None,
            number: 0,
        };
        self.build_key_given(order, row, false, Some(given))
    }

    /// [`Table::build_key`], with `given` as the value of an expression
    /// order's expression instead of evaluating it.
    pub(super) fn build_key_given(
        &self,
        order: &Index,
        mut row: filter::Record,
        condition: bool,
        given: Option<tflip_expr::Val>,
    ) -> Result<Option<Vec<u8>>, Error> {
        let collations = self.session_settings.collations();
        let dynamic = match order.tag.collation.as_str() {
            "" => None,
            name => Some(collations.dynamic(name).ok_or(Error::NotImplemented)?),
        };
        // The static ANSI collation weighs the bytes of 8-bit text.
        let weigh = |key: Vec<u8>| match collations.ansi() {
            Some(ansi) => ansi.key(&key),
            None => key,
        };
        let key_length = usize::from(order.tag.key_length);
        let env = self.env();
        // Dividing by 0 fails the call, which the original engine's codes
        // tell apart by the expression (tools/ace/expr/iif.c).
        let eval = |expr: &Expr, row: &mut filter::Record, division: tflip_expr::Runtime| match expr
            .eval(row, &env)
        {
            Ok(value) => Ok(Some(value)),
            Err(tflip_expr::Failure::Row(error)) => Err(error),
            Err(tflip_expr::Failure::Runtime(tflip_expr::Runtime::DivideByZero)) => {
                Err(Error::Evaluation(division))
            }
            Err(tflip_expr::Failure::Runtime(_)) => Ok(None),
        };
        let filter = order.condition();
        if condition && !filter.is_empty() {
            let filter = self.compile(&filter)?;
            if eval(&filter, &mut row, tflip_expr::Runtime::DivideInCondition)?
                != Some(tflip_expr::Val::Logical(true))
            {
                return Ok(None);
            }
        }
        let value = |row: &mut filter::Record, field: &Field| {
            field
                .value(&row.bytes, row.memo.as_deref_mut())
                .map_err(|_| Error::CorruptTable)
        };
        let expression = |row: &mut filter::Record| {
            if let Some(given) = &given {
                return Ok(given.clone());
            }
            let expression = self.compile(&order.expression())?;
            Ok::<_, Error>(
                eval(&expression, row, tflip_expr::Runtime::DivideInKey)?
                    .unwrap_or(tflip_expr::Val::Null),
            )
        };
        let key = match order.kind {
            tflip_storage::order::Kind::Adi => None,
            tflip_storage::order::Kind::Ntx => Some(match order.key_source(self) {
                seek::KeySource::Field(field) => {
                    let value = value(&mut row, field)?;
                    ntx_field_key(field, &row.bytes, value, key_length, order.tag.decimals)?
                }
                _ => ntx_key(expression(&mut row)?, key_length, order.tag.decimals)?,
            }),
            tflip_storage::order::Kind::Cdx => {
                let nullable = order.tag.flags & tflip_storage::order::flags::NULLABLE != 0;
                let length = key_length - usize::from(nullable);
                // A dynamic collation weighs text without its trailing
                // blanks, padding with zeros; but a segment of a key of
                // several fields keeps them.
                let text = |text: &[u8]| match &dynamic {
                    Some(dynamic) => {
                        let end = text.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
                        dynamic.key(&text[..end], length)
                    }
                    None => tflip_storage::key::text(text, length),
                };
                let segment = |text: &[u8], length: usize| match &dynamic {
                    Some(dynamic) => dynamic.key(text, length),
                    None => tflip_storage::key::text(text, length),
                };
                let key = match order.key_source(self) {
                    seek::KeySource::Field(field) => {
                        let value = value(&mut row, field)?;
                        cdx_field_key(field, &row.bytes, value, length, &text, &segment)?
                    }
                    seek::KeySource::Fields(_) => {
                        let mut key = Vec::with_capacity(key_length);
                        for part in order.tag.expression.split(';') {
                            let number: usize =
                                part.trim().trim_start_matches('F').parse().unwrap_or(0);
                            let field = self.key_field(number)?;
                            let value = value(&mut row, field)?;
                            key.extend(cdx_segment_key(field, &row.bytes, value, &segment)?);
                        }
                        Some(key)
                    }
                    seek::KeySource::Expression => cdx_key(expression(&mut row)?, &text)?,
                };
                Some(match (key, nullable) {
                    (None, true) => Some(vec![0; key_length]),
                    (Some(mut key), true) => {
                        key.insert(0, 0x80);
                        Some(key)
                    }
                    (key, false) => key,
                })
            }
        };
        if let Some(key) = key {
            // A candidate order refuses a NULL key (`fixtures/sql/dbfpk`).
            let candidate = order.tag.flags & tflip_storage::order::flags::CANDIDATE != 0;
            if key.is_none() && candidate {
                return Err(Error::Constraint {
                    code: 5208,
                    text: "The index key cannot be null.".to_owned(),
                });
            }
            return key
                .filter(|key| key.len() == key_length)
                .map(Some)
                .ok_or(Error::NotImplemented);
        }
        let key = match order.key_source(self) {
            seek::KeySource::Field(field) if dynamic.is_some() || is_text(field) => {
                let text = match value(&mut row, field)? {
                    Value::Text(text) => tflip_storage::cp1252::encode(&text),
                    // NULL weighs as blanks in a dynamic order, as zeros
                    // otherwise.
                    Value::Null if dynamic.is_some() => vec![b' '; usize::from(field.length())],
                    Value::Null => return Ok(Some(vec![0; key_length])),
                    _ => return Err(Error::NotImplemented),
                };
                let ci = field.field_type == FieldType::CiCharacter;
                let text = match ci {
                    true => text.to_ascii_lowercase(),
                    false => text,
                };
                Some(match (&dynamic, &env.case_fold) {
                    (Some(dynamic), _) => dynamic.key(&text, key_length),
                    // CICharacter text is in the static collation's lower
                    // case, weighed (`fixtures/write/aofs_static_keys`).
                    (None, Some(fold)) if ci => tflip_storage::key::text(&text, key_length)
                        .iter()
                        .map(|&b| fold.lower[usize::from(b)])
                        .collect(),
                    (None, _) => weigh(tflip_storage::key::text(&text, key_length)),
                })
            }
            seek::KeySource::Field(field) => {
                tflip_storage::key::field_key(field, &value(&mut row, field)?, key_length)
            }
            seek::KeySource::Fields(_) => {
                let mut key = Vec::with_capacity(key_length);
                for part in order.tag.expression.split(';') {
                    let number: usize = part.trim().trim_start_matches('F').parse().unwrap_or(0);
                    let field = self.key_field(number)?;
                    let length =
                        tflip_storage::key::field_key_length_wide(field, order.tag.uchar_width);
                    let part =
                        tflip_storage::key::field_key(field, &value(&mut row, field)?, length)
                            .ok_or(Error::NotImplemented)?;
                    key.extend(part);
                }
                Some(key)
            }
            seek::KeySource::Expression => {
                let expression = self.compile(&order.expression())?;
                match expression.unicode() {
                    tflip_expr::Unicode::No => {
                        let value = match given {
                            Some(given) => given,
                            None => eval(&expression, &mut row, tflip_expr::Runtime::DivideInKey)?
                                .unwrap_or(tflip_expr::Val::Null),
                        };
                        Some(match value {
                            tflip_expr::Val::Null => vec![0; key_length],
                            tflip_expr::Val::Str(text) => match &dynamic {
                                Some(dynamic) => dynamic.key(&text, key_length),
                                None => weigh(tflip_storage::key::text(&text, key_length)),
                            },
                            tflip_expr::Val::Number(n) => tflip_storage::key::double(n).to_vec(),
                            tflip_expr::Val::Logical(b) => tflip_storage::key::logical(b).to_vec(),
                            value => value.bytes(expression.ty()).unwrap_or_default(),
                        })
                    }
                    tflip_expr::Unicode::Unsupported => return Err(Error::NotImplemented),
                    // A value of an expression whose text is Unicode is not
                    // told from 8-bit text by its type alone.
                    _ if given.is_some() => return Err(Error::NotImplemented),
                    _ => {
                        let value = match expression.eval_unicode(&mut row, &env) {
                            Ok(Some(value)) => value,
                            Ok(None) => return Err(Error::NotImplemented),
                            Err(tflip_expr::Failure::Row(error)) => return Err(error),
                            Err(tflip_expr::Failure::Runtime(
                                tflip_expr::Runtime::DivideByZero,
                            )) => {
                                return Err(Error::Evaluation(tflip_expr::Runtime::DivideInKey));
                            }
                            Err(tflip_expr::Failure::Runtime(_)) => tflip_expr::UVal::Null,
                        };
                        let descending = matches!(
                            expression.unicode(),
                            tflip_expr::Unicode::Text {
                                descending: true,
                                ..
                            }
                        );
                        match value {
                            tflip_expr::UVal::Null => Some(vec![0; key_length]),
                            // The complement of the sort key and of the
                            // zeros after it (`fixtures/write/nchar_expr_more`).
                            tflip_expr::UVal::Text(text) if descending => {
                                tflip_storage::icu::nchar_key(&text, key_length)
                                    .map(|key| key.into_iter().map(|b| !b).collect())
                            }
                            tflip_expr::UVal::Text(text) => {
                                tflip_storage::icu::nchar_key(&text, key_length)
                            }
                            // 8-bit text from Unicode text: a key as any
                            // other text's, but for what outgrows it.
                            tflip_expr::UVal::Plain(tflip_expr::Val::Str(text)) => {
                                if text.len() > key_length {
                                    return Err(Error::Evaluation(tflip_expr::Runtime::KeyBuffer));
                                }
                                Some(match &dynamic {
                                    Some(dynamic) => dynamic.key(&text, key_length),
                                    None => weigh(tflip_storage::key::text(&text, key_length)),
                                })
                            }
                            tflip_expr::UVal::Plain(tflip_expr::Val::Number(n)) => {
                                Some(tflip_storage::key::double(n).to_vec())
                            }
                            tflip_expr::UVal::Plain(_) => None,
                        }
                    }
                }
            }
        };
        key.filter(|key| key.len() == key_length)
            .map(Some)
            .ok_or(Error::NotImplemented)
    }

    /// Whether `expression` (fields as `F<n>`) reads a Unicode field.
    pub(crate) fn names_unicode(&self, expression: &str) -> bool {
        let fields = self.fields();
        let bytes = expression.as_bytes();
        (0..bytes.len()).any(|at| {
            let named = bytes[at] == b'F'
                && at.checked_sub(1).is_none_or(|before| {
                    !(bytes[before].is_ascii_alphanumeric() || bytes[before] == b'_')
                });
            let digits = bytes[at + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            named
                && digits > 0
                && expression[at + 1..at + 1 + digits]
                    .parse::<usize>()
                    .is_ok_and(|number| fields.get(number.wrapping_sub(1)).is_some_and(is_unicode))
        })
    }

    /// Adds the current record's key to the custom order `order`
    /// (`AdsAddCustomKey`), built from the record as it is changed; the
    /// order keeps it current from then on. A record with a key in it
    /// already is [`Error::CustomKey`]; an order that is not custom is
    /// refused with 5017, as in the original (`fixtures/errors/custom_kinds.json`).
    pub fn add_custom_key(&self, order: &Index) -> Result<(), Error> {
        self.custom_key(order, true)
    }

    /// Removes the current record's key from the custom order `order`
    /// (`AdsDeleteCustomKey`); nothing happens if it has none.
    pub fn delete_custom_key(&self, order: &Index) -> Result<(), Error> {
        self.custom_key(order, false)
    }

    pub(super) fn custom_key(&self, order: &Index, add: bool) -> Result<(), Error> {
        if !order.tag.custom() {
            return Err(Error::FullText(
                crate::ace_error::codes::AE_INVALID_INDEX_TYPE,
            ));
        }
        let mut state = self.state();
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        self.check_writable(&state)?;
        let length = self.structure.header.record_length as usize;
        let bytes = state.read(record, length)?;
        let key = state
            .with_memo(|memo| {
                let row = filter::Record {
                    fields: self.fields(),
                    bytes,
                    memo,
                    number: record,
                };
                self.build_key(order, row, false)
            })?
            .ok_or(Error::NotImplemented)?;
        let shared = Arc::clone(&state.file);
        let mut file = shared.file();
        file.begin_write().map_err(write_error)?;
        let changed = (|| {
            let (index, mut guard) = order_file(order)?;
            let index_error = |_| Error::CorruptIndex;
            match &mut *guard {
                OrderFile::Adi(index_file) => {
                    let has = index_file
                        .has_key(&order.tag, &key, Some(record))
                        .map_err(index_error)?;
                    match (add, has) {
                        (true, true) => return Err(Error::CustomKey),
                        (true, false) => index_file
                            .insert_key(&order.tag, &key, record)
                            .map_err(index_error)?,
                        (false, true) => index_file
                            .delete_key(&order.tag, &key, record)
                            .map_err(index_error)?,
                        (false, false) => return Ok(()),
                    }
                    index_file.count_change(&order.tag);
                }
                OrderFile::Cdx(index_file) => {
                    let has = index_file
                        .has_key(&order.tag, &key, Some(record))
                        .map_err(index_error)?;
                    match (add, has) {
                        (true, true) => return Err(Error::CustomKey),
                        (true, false) => index_file
                            .insert_key(&order.tag, &key, record)
                            .map_err(index_error)?,
                        (false, true) => {
                            index_file
                                .delete_key(&order.tag, &key, record)
                                .map_err(index_error)?;
                        }
                        (false, false) => return Ok(()),
                    }
                    index_file.count_changes(1).map_err(index_error)?;
                }
                OrderFile::Ntx(_) => return Err(Error::NotImplemented),
            }
            index.changed();
            Ok(())
        })();
        let ended = file.end_write().map_err(write_error);
        changed.and(ended)
    }

    /// The key of the record `record` (a whole record's bytes) in `order`
    /// (`AdsBuildKeyFromRecord`), whatever the order's condition.
    pub fn key_from_record(&self, order: &Index, record: &[u8]) -> Result<Vec<u8>, Error> {
        let length = self.structure.header.record_length as usize;
        if record.len() != length || length == 0 {
            return Err(Error::InvalidRecordLength);
        }
        let row = filter::Record {
            fields: self.fields(),
            bytes: record.to_vec(),
            memo: None,
            number: 0,
        };
        self.build_key(order, row, false)?
            .ok_or(Error::NotImplemented)
    }

    /// Whether `order` has the key `value` seeks, the whole key (padded to
    /// the order's key length), whatever the scope and the filters
    /// (`AdsLookupKey`). The table does not move.
    pub fn lookup_key(&self, order: &Index, value: SeekValue) -> Result<bool, Error> {
        let mut key = self.sought_key(order, value)?;
        key.resize(usize::from(order.tag.key_length), order.tag.pad);
        Ok(order.seek(&key)?.is_some_and(|(_, found, _)| found))
    }

    /// The key of the record `record` (a whole record's bytes) in `order`,
    /// whatever its condition, as [`Table::build_key`] builds it.
    pub(crate) fn key_of_bytes(
        &self,
        order: &Index,
        record: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, Error> {
        let row = filter::Record {
            fields: self.fields(),
            bytes: record,
            memo: None,
            number: 0,
        };
        self.build_key(order, row, false)
    }

    /// Applies `value` to `field` of `record` (a whole record's bytes, not
    /// the table's), as [`Table::set_field`] does to the current record but
    /// for memo values; for an order's raw key (`AdsInitRawKey`).
    pub(crate) fn set_in_bytes(
        &self,
        field: FieldRef,
        value: &SetValue,
        record: &mut [u8],
    ) -> Result<(Field, usize), Error> {
        let (number, field) = find_field(self.fields(), field)?;
        let field = field.clone();
        if field.is_memo() {
            return Err(Error::InvalidFieldType);
        }
        let date_format = self.date_format();
        let dates = setters::Dates {
            format: &date_format,
            epoch: self.session_settings.epoch(),
        };
        match setters::apply(&field, value, dates, record) {
            Ok(()) | Err(Error::DataTruncated) => Ok((field, number)),
            Err(error) => Err(error),
        }
    }
}

/// Whether `field` holds Unicode text (NChar, NVarChar), whose keys are ICU
/// sort keys.
pub(super) fn is_unicode(field: &Field) -> bool {
    matches!(field.field_type, FieldType::NChar | FieldType::NVarChar)
}

/// Whether `field` holds 8-bit text, whose keys a collation weighs.
pub(super) fn is_text(field: &Field) -> bool {
    use FieldType::*;
    matches!(
        field.field_type,
        Character | CiCharacter | VarChar | LegacyVarChar
    )
}

/// The bytes a DBF field takes in `record`, which the keys of character
/// fields are (of an ADS VarChar, the text inline, which the key is as long
/// as).
pub(super) fn stored<'r>(field: &Field, record: &'r [u8], length: usize) -> &'r [u8] {
    let start = usize::from(field.offset);
    &record[start..start + length.min(usize::from(field.size))]
}

/// The key of `field` in an NTX order: text, as Clipper's indexes keep it.
/// Character fields give their stored bytes, numbers their digits (see
/// [`tflip_storage::ntx::number_key`]; blank is 0), dates `YYYYMMDD` (blank:
/// spaces) and logical fields `T` or `F`. `None` for other types.
pub(super) fn ntx_field_key(
    field: &Field,
    record: &[u8],
    value: Value,
    key_length: usize,
    decimals: u16,
) -> Result<Option<Vec<u8>>, Error> {
    use FieldType::*;
    let number = |n: f64| ntx_number_key(n, key_length, decimals);
    Ok(Some(match (field.field_type, value) {
        (Character | LegacyVarChar | CiCharacter, _) => stored(field, record, key_length).to_vec(),
        (Numeric, Value::Numeric(text)) => number(text.trim().parse().unwrap_or(0.0))?,
        (Integer | ShortInt | AutoInc, Value::Integer(n)) => number(n as f64)?,
        (Double | CurDouble, Value::Double(n)) => number(n)?,
        (Numeric | Integer | ShortInt | AutoInc | Double | CurDouble, _) => number(0.0)?,
        (Date | CompactDate, Value::Date(julian)) => tflip_storage::ntx::date_key(julian),
        (Date | CompactDate, _) => tflip_storage::ntx::date_key(0),
        (Logical, _) => match record[usize::from(field.offset)] {
            b'T' | b't' | b'Y' | b'y' => b"T".to_vec(),
            _ => b"F".to_vec(),
        },
        (Guid, _) => stored(field, record, key_length).to_vec(),
        _ => return Ok(None),
    }))
}

/// The NTX key of the number `n` (see [`tflip_storage::ntx::number_key`]),
/// or [`Error::KeyOverflow`] if it does not fit the key: the original engine
/// refuses to write a record whose key it cannot build.
pub(super) fn ntx_number_key(n: f64, key_length: usize, decimals: u16) -> Result<Vec<u8>, Error> {
    let key = tflip_storage::ntx::number_key(n, key_length, usize::from(decimals));
    match key.iter().all(|&b| b == b'*') {
        true => Err(Error::KeyOverflow),
        false => Ok(key),
    }
}

/// The NTX key of an expression's `value`: text padded with spaces to the
/// key's length, numbers as a Numeric field's, dates and logical values as
/// their fields'. `None` for other types.
pub(super) fn ntx_key(
    value: tflip_expr::Val,
    key_length: usize,
    decimals: u16,
) -> Result<Option<Vec<u8>>, Error> {
    use tflip_expr::Val;
    Ok(Some(match value {
        Val::Str(text) => tflip_storage::key::text(&text, key_length),
        Val::Number(n) => ntx_number_key(n, key_length, decimals)?,
        Val::Date(julian) => tflip_storage::ntx::date_key(julian),
        Val::Logical(b) => if b { b"T" } else { b"F" }.to_vec(),
        _ => return Ok(None),
    }))
}

/// The key of `field` in a CDX order, `length` bytes without the NULL
/// prefix (which a NULL field does not get: `None` for it), or
/// [`Error::NotImplemented`] for a type whose keys tflip does not build:
/// binary sort keys, compared as bytes. Character fields give their text (`text` pads
/// or weighs it), numbers and dates an order-preserving double (blank: 0),
/// a VFP Integer or AutoInc four bytes and Money eight, logical fields `T`
/// or `F`.
pub(super) fn cdx_field_key(
    field: &Field,
    record: &[u8],
    value: Value,
    length: usize,
    text: &dyn Fn(&[u8]) -> Vec<u8>,
    segment: &dyn Fn(&[u8], usize) -> Vec<u8>,
) -> Result<Option<Vec<u8>>, Error> {
    use FieldType::*;
    if value == Value::Null && matches!(&field.layout, Layout::Dbf(l) if l.null_bit.is_some()) {
        return Ok(None);
    }
    Ok(Some(match (field.field_type, &value) {
        (Integer | AutoInc, Value::Integer(n)) if length == 4 => sortable(*n as i64, 4),
        (Integer | AutoInc, _) if length == 4 => sortable(0, 4),
        (Character | LegacyVarChar | CiCharacter, _) => {
            text(stored(field, record, usize::from(field.length())))
        }
        (VarChar, Value::Text(t)) => text(&tflip_storage::cp1252::encode(t)),
        (VarChar, _) => text(b""),
        _ => cdx_segment_key(field, record, value, segment)?,
    }))
}

/// The key of `field` as a segment of a multi-field CDX key: as
/// [`cdx_field_key`], but a VFP Integer is a double, and a NULL value all
/// zeros.
pub(super) fn cdx_segment_key(
    field: &Field,
    record: &[u8],
    value: Value,
    text: &dyn Fn(&[u8], usize) -> Vec<u8>,
) -> Result<Vec<u8>, Error> {
    use FieldType::*;
    use tflip_storage::key;
    let nullable = matches!(&field.layout, Layout::Dbf(l) if l.null_bit.is_some());
    let double = |n: f64| key::double(n).to_vec();
    Ok(match (field.field_type, value) {
        (_, Value::Null) if nullable => vec![0; cdx_segment_length(field)],
        (Character | LegacyVarChar | CiCharacter, _) => {
            let length = usize::from(field.length());
            text(stored(field, record, length), length)
        }
        (VarChar, Value::Text(t)) => text(
            &tflip_storage::cp1252::encode(&t),
            usize::from(field.length()),
        ),
        (VarChar, _) => text(b"", usize::from(field.length())),
        (Numeric, Value::Numeric(t)) => double(t.trim().parse().unwrap_or(0.0)),
        (Integer | ShortInt | AutoInc | LongInt, Value::Integer(n)) => double(n as f64),
        (Double | CurDouble, Value::Double(n)) => double(n),
        (Numeric | Integer | ShortInt | AutoInc | LongInt | Double | CurDouble, _) => double(0.0),
        (Money, Value::Money(n)) => sortable(n, 8),
        (Money, _) => sortable(0, 8),
        (Date, Value::Date(julian)) => double(f64::from(julian)),
        (Date, _) => double(0.0),
        // A VFP timestamp's key is a double of the day and its fraction,
        // rounded as the original rounds it.
        (TimeStamp, Value::TimeStamp(julian, ms)) => double(key::vfp_timestamp(julian, ms)),
        (TimeStamp, _) => double(0.0),
        (Logical, _) => match record[usize::from(field.offset)] {
            b'T' | b't' | b'Y' | b'y' => b"T".to_vec(),
            _ => b"F".to_vec(),
        },
        _ => return Err(Error::NotImplemented),
    })
}

/// How long a field's segment of a multi-field CDX key is.
pub(crate) fn cdx_segment_length(field: &Field) -> usize {
    use FieldType::*;
    match field.field_type {
        Character | LegacyVarChar | CiCharacter | VarChar => usize::from(field.length()),
        Logical => 1,
        _ => 8,
    }
}

/// The CDX key of an expression's `value`, without the NULL prefix: text
/// padded (or weighed) by `text`, numbers, dates and timestamps as
/// order-preserving doubles, logical values `T` or `F`; `None` for NULL.
pub(super) fn cdx_key(
    value: tflip_expr::Val,
    text: &dyn Fn(&[u8]) -> Vec<u8>,
) -> Result<Option<Vec<u8>>, Error> {
    use tflip_expr::Val;
    Ok(Some(match value {
        Val::Null => return Ok(None),
        Val::Str(t) => text(&t),
        Val::Number(n) => tflip_storage::key::double(n).to_vec(),
        Val::Date(julian) => tflip_storage::key::double(f64::from(julian)).to_vec(),
        // As a VFP TimeStamp field's key, but the milliseconds left out,
        // not rounded (`fixtures/write/cdx_timestamps`).
        Val::TimeStamp(julian, ms) => {
            let day = tflip_storage::key::vfp_timestamp(julian, ms - ms % 1000);
            tflip_storage::key::double(day).to_vec()
        }
        Val::Logical(b) => if b { b"T" } else { b"F" }.to_vec(),
        _ => return Err(Error::NotImplemented),
    }))
}

/// How a seek through a cursor's temporary index looks for its key.
#[derive(Clone, Copy)]
pub(super) enum Find {
    Seek(SeekType),
    Last,
}

impl Table {
    /// The records an SQL cursor shows, in its order, with their keys in its
    /// temporary index `order`. An order whose keys tflip cannot build is
    /// [`Error::NotImplemented`].
    fn temporary_keys(&self, order: &Index) -> Result<Vec<(u32, Vec<u8>)>, Error> {
        let sieve = self.sieve();
        let mut shown = Vec::new();
        {
            let mut state = self.state();
            for slot in state.natural() {
                if state.visible(slot, &sieve)? {
                    shown.push(slot);
                }
            }
        }
        shown
            .into_iter()
            .map(|slot| {
                let key = self
                    .built_key(order, slot)
                    .map_err(|_| Error::NotImplemented)?;
                Ok((slot, key.ok_or(Error::NotImplemented)?))
            })
            .collect()
    }

    /// `AdsSeek` and `AdsSeekLast` through a cursor's temporary index, which
    /// has no file: the keys are those of the records in the order the cursor
    /// shows them (`fixtures/sql/orderby.json`). A key is found when a key
    /// starts with it; a seek that finds none goes to EOF, and so does a
    /// value that cannot be converted to a key. `gt` in a descending index
    /// lands on the first key at or below the sought key's successor, so it
    /// finds the last of the keys that start with the sought one.
    pub(super) fn find_temporary(
        &self,
        order: &Index,
        value: SeekValue,
        find: Find,
    ) -> Result<bool, Error> {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let keys = self.temporary_keys(order)?;
        let key = match self.sought_key(order, value) {
            Ok(key) => key,
            Err(error) => {
                self.state().land_in(None, true, false)?;
                return Err(error);
            }
        };
        let descending = order.tag.descending;
        let starts = |have: &[u8], sought: &[u8]| have[..sought.len().min(have.len())].cmp(sought);
        let at = match (find, descending) {
            (Find::Seek(SeekType::Soft | SeekType::Hard), _) if key.is_empty() => {
                (!keys.is_empty()).then_some(0)
            }
            (Find::Seek(SeekType::Soft | SeekType::Hard), false) => {
                keys.iter().position(|(_, have)| starts(have, &key) != Less)
            }
            (Find::Seek(SeekType::Soft | SeekType::Hard), true) => keys
                .iter()
                .position(|(_, have)| starts(have, &key) != Greater),
            (Find::Seek(SeekType::After), false) => keys
                .iter()
                .position(|(_, have)| starts(have, &key) == Greater),
            (Find::Seek(SeekType::After), true) => {
                let mut next = key.clone();
                while next.last() == Some(&0xFF) {
                    next.pop();
                }
                match next.last_mut() {
                    Some(last) => {
                        *last += 1;
                        keys.iter()
                            .position(|(_, have)| starts(have, &next) != Greater)
                    }
                    None => None,
                }
            }
            (Find::Last, _) => keys
                .iter()
                .rposition(|(_, have)| starts(have, &key) == Equal),
        };
        let found = at.is_some_and(|i| match find {
            Find::Seek(SeekType::After) | Find::Last => true,
            Find::Seek(_) => key.is_empty() || starts(&keys[i].1, &key) == Equal,
        });
        let landed = at
            .filter(|_| found || matches!(find, Find::Seek(SeekType::Soft)))
            .map(|i| keys[i].0);
        // A last key sought before every key is also before the first.
        let before_all = matches!(find, Find::Last)
            && !found
            && keys.first().is_some_and(|(_, first)| {
                starts(first, &key) == if descending { Less } else { Greater }
            });
        let mut state = self.state();
        state.land_in(landed, true, before_all)?;
        state.found = found;
        Ok(found)
    }
}

/// The keys of `row` in the full-text order `tag`, which tokenizes with
/// `rules`: each distinct word of its field, in its index's encoding, cut to
/// and padded to the key's length, with how often it occurs (words that are
/// the same once cut count together), in the order of their bytes. None for
/// a NULL field.
pub(crate) fn words(
    tag: &tflip_storage::order::Tag,
    rules: &tflip_sql::fts::Rules,
    row: &mut filter::Record,
) -> Result<Vec<(Vec<u8>, u16)>, Error> {
    use tflip_expr::Row;
    // An ADI order's expression is the field's number (`F3`), a CDX one's
    // its name (`fixtures/write/fts_cdx`).
    let expression = tag.expression.trim();
    let number: usize = match expression
        .strip_prefix('F')
        .and_then(|digits| digits.parse().ok())
    {
        Some(number) => number,
        None => {
            1 + row
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(expression))
                .ok_or(Error::CorruptIndex)?
        }
    };
    // A Binary, Image or VarBinary field's bytes are words like any other
    // text, but a zero byte splits them whatever the delimiters
    // (`fixtures/write/fts_binary`).
    let texts: Vec<String> = match row.value(number.wrapping_sub(1))? {
        Value::Text(text) => vec![text],
        Value::Binary(bytes) => bytes
            .split(|&b| b == 0)
            .map(tflip_storage::cp1252::decode)
            .collect(),
        // A VFP field set to NULL keeps its bytes, and its words with them:
        // the original engine reads the text and not the flag, so the order
        // keeps the record's words (`fixtures/write/fts_cdx_null`).
        Value::Null => match row.fields.get(number.wrapping_sub(1)) {
            Some(field)
                if field.field_type == tflip_storage::field::FieldType::Character
                    && matches!(&field.layout,
                        tflip_storage::field::Layout::Dbf(l) if l.null_bit.is_some()) =>
            {
                vec![tflip_storage::cp1252::decode(field.bytes(&row.bytes))]
            }
            _ => return Ok(Vec::new()),
        },
        _ => return Err(Error::NotImplemented),
    };
    let length = usize::from(tag.key_length);
    let mut counts: std::collections::BTreeMap<Vec<u8>, u16> = std::collections::BTreeMap::new();
    for word in texts
        .iter()
        .flat_map(|text| tflip_sql::fts::tokenize(text, rules))
    {
        let mut key = tflip_storage::cp1252::encode(&word);
        key.truncate(length);
        key.resize(length, tag.pad);
        let count = counts.entry(key).or_default();
        *count = count.saturating_add(1);
    }
    Ok(counts.into_iter().collect())
}
