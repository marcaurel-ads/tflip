//! Filters of a table: its Advantage Optimized Filter (set, optimized,
//! resolved through the index orders, customized and counted), its record
//! filter, and `AdsLocate`.

use super::*;

impl Table {
    /// Sets the table's Advantage Optimized Filter (`AdsSetAOF`), replacing
    /// any it had, optimized with the index orders `orders` (those open on
    /// the table's handle), and resolved as `options` say (see
    /// [`crate::filter`]). It is kept with the field names as the table
    /// defines them, as the original engine keeps it (`ID > 2` becomes
    /// `id > 2`). An expression that is not valid is [`Error::Expression`]
    /// (an empty one [`Error::InvalidExpression`]), one that is not logical
    /// [`Error::AofNotLogical`].
    pub fn set_aof(
        &self,
        expression: &str,
        options: AofOptions,
        orders: &[Arc<Index>],
    ) -> Result<(), Error> {
        let set = self.try_set_aof(expression, options, orders);
        if set.is_err() {
            // The AOF it had stays for moves alone (see [`Aof::orphaned`]).
            self.change_filters(|filters| {
                if let Some(aof) = &mut filters.aof {
                    aof.orphaned = true;
                }
            });
        }
        set
    }

    pub(super) fn try_set_aof(
        &self,
        expression: &str,
        options: AofOptions,
        orders: &[Arc<Index>],
    ) -> Result<(), Error> {
        let expr = self.compile(expression)?;
        if !matches!(expr.ty(), Type::Logical | Type::Null) {
            return Err(Error::AofNotLogical);
        }
        let text = self.with_field_names(&self.with_field_numbers(expression))?;
        let (mut optimized, serving) = self.optimize_with(expression, orders, &[])?;
        // `YEAR()` terms the orders find are evaluated as they find them.
        let expr = match self.index_bounds(expression, &mut optimized.bound, &serving)? {
            Some(evaluated) => self.compile(&evaluated)?,
            None => expr,
        };
        let contains = self.contains_hook(&expr, orders)?;
        let aof = Aof {
            filter: Filter {
                text,
                expr: Arc::new(expr),
                contains,
            },
            options,
            level: optimized.level,
            residual: optimized.residual,
            bound: Arc::new(optimized.bound),
            members: Arc::default(),
            orphaned: false,
        };
        self.resolve_aof(aof, &serving)
    }

    /// How far `expression` is optimized as an AOF with the index orders
    /// `orders`: those of them that serve (not NTX orders, nor orders with a
    /// condition, custom, full-text or binary ones, nor unique orders of DBF
    /// tables, which the original engine does not trust to be unique).
    pub fn optimize(
        &self,
        expression: &str,
        orders: &[Arc<Index>],
    ) -> Result<tflip_expr::optimize::Optimized, Error> {
        Ok(self.optimize_with(expression, orders, &[])?.0)
    }

    /// [`Table::optimize`], and the orders that serve, which the
    /// optimization's [`Bound`](tflip_expr::optimize::Bound) numbers; `fts`
    /// are the expression's full-text terms (an SQL `WHERE`'s `CONTAINS`).
    pub(crate) fn optimize_with(
        &self,
        expression: &str,
        orders: &[Arc<Index>],
        fts: &[tflip_expr::optimize::FtsTerm],
    ) -> Result<(tflip_expr::optimize::Optimized, Vec<Arc<Index>>), Error> {
        use tflip_storage::order::{Kind, flags};
        if expression.is_empty() {
            return Err(Error::InvalidExpression);
        }
        let serves = |order: &Index| {
            let tag = &order.tag;
            order.kind != Kind::Ntx
                && tag.condition.is_empty()
                && !tag.custom()
                && !tag.full_text()
                && !tag.binary()
                && (order.kind == Kind::Adi || !tag.unique() && tag.flags & flags::CANDIDATE == 0)
        };
        let (mut keys, mut serving) = (Vec::new(), Vec::new());
        for order in orders.iter().filter(|order| serves(order)) {
            if let Ok(key) = self.compile(&order.expression()) {
                keys.push(key);
                serving.push(Arc::clone(order));
            }
        }
        let alias = self.expression_alias();
        let schema = filter::Schema {
            alias: &alias,
            fields: self.fields(),
        };
        let collated: Vec<bool> = serving
            .iter()
            .map(|order| !order.tag.collation.is_empty())
            .collect();
        let orders = tflip_expr::optimize::Orders {
            keys: &keys,
            collated: &collated,
            negation_found: self.format() != Format::Adt,
            fts,
        };
        let optimized = tflip_expr::optimize::optimize(expression, &schema, &orders)
            .map_err(Error::Expression)?;
        Ok((optimized, serving))
    }

    /// Resolves `aof` as setting it does, finding records through `orders`
    /// (those its bound numbers), and makes it the table's AOF: an AOF
    /// resolved at once or fully optimized knows its records and counts
    /// them; another has the records the orders leave as candidates. A fixed
    /// one is frozen from the start.
    pub(super) fn resolve_aof(&self, mut aof: Aof, orders: &[Arc<Index>]) -> Result<(), Error> {
        let exact = aof.exact();
        let mut members = Members {
            frozen: aof.options.fixed,
            ..Members::default()
        };
        {
            // The table may have no filters yet: evaluate with its settings.
            let mut sieve = self.sieve();
            sieve.env = self.env();
            let mut state = self.state();
            let mut active = Bits::default();
            for slot in 1..=state.slots() {
                match state.active(slot)? {
                    true => active.set(slot, true),
                    false => members.gone.set(slot, true),
                }
            }
            let found = self.bound_records(&aof.bound, orders, &sieve, &mut state)?;
            // With no candidates, the original engine reports nothing left
            // to evaluate.
            if aof.level == tflip_expr::optimize::Level::Partial
                && found.as_ref().is_some_and(|found| found.count() == 0)
            {
                aof.residual.clear();
            }
            let candidates = found.unwrap_or(active);
            let superset = if exact {
                let mut inside = Bits::default();
                for slot in candidates.records() {
                    let bytes = state.read(slot, sieve.record_length)?;
                    let holds =
                        state.with_memo(|memo| sieve.holds(&aof.filter.expr, bytes, slot, memo))?;
                    inside.set(slot, holds);
                }
                members.count = Some(inside.count());
                if aof.options.fixed {
                    members.inside = inside.clone();
                }
                inside
            } else {
                members.unresolved = candidates.clone();
                candidates
            };
            members.superset = Some((state.changes()?, superset));
        }
        aof.members = Arc::new(Mutex::new(members));
        self.change_filters(|filters| {
            filters.aof = Some(aof);
            filters.parked = None;
        });
        Ok(())
    }

    /// The records `bound` leaves as candidates, which are active: found
    /// through `orders` where an order serves a comparison (an ADI order on
    /// one field), else by evaluating the part on every record; `None` for
    /// any record.
    pub(super) fn bound_records(
        &self,
        bound: &tflip_expr::optimize::Bound,
        orders: &[Arc<Index>],
        sieve: &Sieve,
        state: &mut State,
    ) -> Result<Option<Bits>, Error> {
        use tflip_expr::optimize::Bound;
        Ok(match bound {
            Bound::All => None,
            // A constant: true for any record, or for none.
            Bound::Exact {
                expr, order: None, ..
            } => {
                let bytes = self.null_record.clone();
                match sieve.holds(expr, bytes, 0, None)? {
                    true => None,
                    false => Some(Bits::default()),
                }
            }
            Bound::Exact {
                expr,
                order: Some(order),
                compare,
            } => {
                let found = match (orders.get(*order), compare) {
                    (Some(order), Some(compare)) => {
                        self.term_records(order, compare, &sieve.env)?
                    }
                    _ => None,
                };
                let slots: Vec<u32> = match found {
                    Some(records) => records,
                    None => (1..=state.slots()).collect(),
                };
                let mut bits = Bits::default();
                for slot in slots {
                    if slot == 0 || slot > state.slots() || !state.active(slot)? {
                        continue;
                    }
                    let bytes = state.read(slot, sieve.record_length)?;
                    if state.with_memo(|memo| sieve.holds(expr, bytes, slot, memo))? {
                        bits.set(slot, true);
                    }
                }
                Some(bits)
            }
            Bound::And(a, b) => {
                let a = self.bound_records(a, orders, sieve, state)?;
                match (a, self.bound_records(b, orders, sieve, state)?) {
                    (Some(a), Some(b)) => Some(a.and(&b)),
                    (Some(found), None) | (None, Some(found)) => Some(found),
                    (None, None) => None,
                }
            }
            Bound::Or(a, b) => {
                let a = self.bound_records(a, orders, sieve, state)?;
                match (a, self.bound_records(b, orders, sieve, state)?) {
                    (Some(a), Some(b)) => Some(a.or(&b)),
                    _ => None,
                }
            }
            Bound::Fts(records) => Some(Bits::of(records)),
            Bound::Not(a) => match self.bound_records(a, orders, sieve, state)? {
                Some(found) => {
                    let mut bits = Bits::default();
                    for slot in 1..=state.slots() {
                        if !found.get(slot) && state.active(slot)? {
                            bits.set(slot, true);
                        }
                    }
                    Some(bits)
                }
                None => None,
            },
        })
    }

    /// The records whose keys in `order` may compare with a value as
    /// `compare` says, a few more perhaps: `None` where the order cannot
    /// tell (an NTX order, text keys weighed by a dynamic collation the
    /// table's expressions do not compare by, CICharacter text under a
    /// static collation but of an ADI order on the field, a value of another
    /// type, a `YEAR()` term, which [`Table::index_bounds`] makes plain
    /// comparisons of).
    ///
    /// A comparison finds the keys in a range: text as far as the value goes
    /// (the exact setting off) or without its trailing blanks, so that a
    /// bound takes in every key that starts with the value's text; with a
    /// key of several fields, every key whose first field (or leading part)
    /// compares. `<>` finds the records but those whose key is the value's;
    /// `$` the keys in the value's text, `EMPTY()` and `ISNULL()` the keys
    /// of empty or NULL values (see [`Table::within_records`],
    /// [`Table::empty_records`]).
    pub(super) fn term_records(
        &self,
        order: &Index,
        compare: &tflip_expr::optimize::Compare,
        env: &Env,
    ) -> Result<Option<Vec<u32>>, Error> {
        self.term_records_as(order, compare, env, false)
    }

    /// [`Table::term_records`]; for `sql`, a superset of the records SQL
    /// finds, which compares text by its bytes: `None` for text under the
    /// static collation, `$`, `EMPTY()` and leading parts of keys.
    fn term_records_as(
        &self,
        order: &Index,
        compare: &tflip_expr::optimize::Compare,
        env: &Env,
        sql: bool,
    ) -> Result<Option<Vec<u32>>, Error> {
        use tflip_expr::optimize::Comparison;
        use tflip_expr::{Type, Val, eval_constant};
        use tflip_sql::V;
        if sql
            && (compare.lead.is_some()
                || matches!(
                    compare.op,
                    Comparison::Within | Comparison::Empty | Comparison::Null
                ))
        {
            return Ok(None);
        }
        use tflip_storage::order::Kind;
        if order.kind == Kind::Ntx || compare.year.is_some() {
            return Ok(None);
        }
        match compare.op {
            Comparison::Within => return self.within_records(order, compare, env),
            Comparison::Empty | Comparison::Null => return self.empty_records(order, compare, env),
            _ => {}
        }
        let Some(value) = eval_constant(&compare.value, env) else {
            return Ok(None);
        };
        let given = value.clone();
        let key_length = usize::from(order.tag.key_length);
        // How a bound's key is built: the value's own (`Exact`), or the
        // nearest the field holds below it or above it.
        #[derive(Clone, Copy, PartialEq)]
        enum Side {
            Low,
            High,
            Exact,
        }
        type KeyOf<'a> = Box<dyn Fn(Side) -> Result<Option<Vec<u8>>, Error> + 'a>;
        // The key of the value, whether keys are text, and the length of the
        // part of the key the term compares (the first field's of a key of
        // several fields).
        let (key, text, segment): (KeyOf, bool, usize) = match order.key_source(self) {
            source @ (seek::KeySource::Field(field) | seek::KeySource::Fields(field)) => {
                let several = matches!(source, seek::KeySource::Fields(_));
                // A CDX key of several fields is text built otherwise.
                if several && order.kind != Kind::Adi {
                    return Ok(None);
                }
                let Some(number) = self
                    .fields()
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(&field.name))
                else {
                    return Ok(None);
                };
                let text = matches!(
                    field.field_type,
                    FieldType::Character | FieldType::CiCharacter | FieldType::VarChar
                );
                let v = match value {
                    Val::Number(n) => V::Double(n),
                    Val::Date(day) => V::Date(day),
                    Val::Logical(b) => V::Bool(b),
                    Val::Time(ms) => V::Time(ms),
                    Val::TimeStamp(day, ms) => V::TimeStamp(day, ms),
                    Val::Str(bytes) if text => V::Str(tflip_storage::cp1252::decode(&bytes)),
                    _ => return Ok(None),
                };
                let segment = match several {
                    true => tflip_storage::key::field_key_length_wide(field, order.tag.uchar_width),
                    false => key_length,
                };
                let key = move |side: Side| {
                    let value = match side {
                        Side::Exact => exact_value(field, &v),
                        side => crate::statement::bound_value(field, &v, side == Side::High),
                    };
                    let Some(value) = value else { return Ok(None) };
                    self.value_key(order, number, &value)
                };
                (Box::new(key), text, segment)
            }
            // Expressions of several segments are not looked up.
            seek::KeySource::Expression if order.expression().contains(';') => return Ok(None),
            seek::KeySource::Expression => {
                let ty = self.compile(&order.expression())?.ty();
                let text = match (&ty, &value) {
                    (Type::String { .. }, Val::Str(_)) => true,
                    (Type::Number(_), Val::Number(_))
                    | (Type::Date, Val::Date(_))
                    | (Type::Logical, Val::Logical(_)) => false,
                    _ => return Ok(None),
                };
                // CICharacter text is in lower case in keys and compares
                // so.
                let value = match (ty, value) {
                    (Type::String { ci: true, .. }, Val::Str(t)) => {
                        Val::Str(t.to_ascii_lowercase())
                    }
                    (_, value) => value,
                };
                let key = move |_: Side| match self.expression_value_key(order, value.clone()) {
                    // A key that cannot be built tells nothing.
                    Err(Error::NotImplemented) => Ok(None),
                    found => found,
                };
                // A term on a leading part of the key compares that part.
                (
                    Box::new(key),
                    text,
                    compare.lead.unwrap_or(key_length).min(key_length),
                )
            }
        };
        // Keys are ordered as bytes, but text by a collation's weights. Text
        // under an ADI order's dynamic collation is found when the
        // table's expressions compare by the same collation, as the
        // original engine's do.
        let collations = self.session_settings.collations();
        let ansi = collations.ansi();
        let dynamic = match order.tag.collation.as_str() {
            "" => None,
            name => Some(name),
        };
        let compared = env.dynamic.as_ref().map(|d| d.name.as_str());
        let by_dynamic = text
            && order.kind == Kind::Adi
            && segment == key_length
            && dynamic
                .zip(compared)
                .is_some_and(|(a, b)| a.trim().eq_ignore_ascii_case(b));
        // Text under the static collation: an ADI order's keys are its
        // weights, a CDX order's the bytes its cursor compares by them; the
        // expressions compare text by them too. CICharacter text, which the
        // expressions fold by the collation first, is found in an ADI order
        // on the field, whose keys are folded so too; elsewhere it is
        // evaluated.
        let ci = match order.key_source(self) {
            seek::KeySource::Field(field) => {
                field.field_type == FieldType::CiCharacter && order.kind != Kind::Adi
            }
            seek::KeySource::Fields(field) => field.field_type == FieldType::CiCharacter,
            seek::KeySource::Expression => {
                matches!(
                    self.compile(&order.expression())?.ty(),
                    Type::String { ci: true, .. }
                )
            }
        };
        let by_static = !sql
            && !ci
            && ansi.is_some()
            && text
            && dynamic.is_none()
            && !matches!(order.key_source(self), seek::KeySource::Fields(_));
        // Other keys compare as bytes, but those of a CDX order of a dynamic
        // collation.
        if (ansi.is_some() || dynamic.is_some())
            && !by_dynamic
            && !by_static
            && (text || order.kind != Kind::Adi && dynamic.is_some())
        {
            return Ok(None);
        }
        // What fills a text key after its text: blanks (their weight in an
        // ADI order under the static collation), or a dynamic collation's
        // zeros.
        let pad = match ansi {
            _ if by_dynamic => 0,
            Some(ansi) if by_static && order.kind == Kind::Adi => ansi.weights[usize::from(b' ')],
            _ => b' ',
        };
        // What fills a bound from below and from above: the bytes a CDX
        // order weighs least and most under the static collation.
        let (least, most) = match order.weights().filter(|_| by_static) {
            Some(w) => {
                let least = (0..=255u8).min_by_key(|&b| w[usize::from(b)]).unwrap_or(0);
                let most = (0..=255u8)
                    .max_by_key(|&b| w[usize::from(b)])
                    .unwrap_or(0xFF);
                (least, most)
            }
            None => (0, 0xFF),
        };
        // The key `side` builds, its part past the first `end` bytes of the
        // segment filled for a bound from below (zeros) or above.
        let bound = |side: Side, end: Option<usize>| -> Result<Option<Vec<u8>>, Error> {
            let Some(mut key) = key(side)? else {
                return Ok(None);
            };
            if key.len() < segment {
                return Ok(None);
            }
            let end = end.unwrap_or_else(|| match text {
                true => key[..segment]
                    .iter()
                    .rposition(|&b| b != pad)
                    .map_or(0, |i| i + 1),
                false => segment,
            });
            key[end..].fill(if side == Side::High { most } else { least });
            Ok(Some(key))
        };
        let (low, high) = match compare.op {
            Comparison::Eq | Comparison::Exactly => (true, true),
            Comparison::Lt | Comparison::Le => (false, true),
            Comparison::Gt | Comparison::Ge => (true, false),
            Comparison::Within | Comparison::Empty | Comparison::Null => return Ok(None),
            Comparison::Ne => {
                // The records but those whose key is exactly the value's: its
                // text (with its blanks) or number, which `=` holds for
                // whatever the exact setting.
                let all = order.records_between(None, None)?;
                let Some(equal) = bound(Side::Exact, Some(segment))? else {
                    return Ok(Some(all));
                };
                let high = bound(Side::Exact, Some(segment))?.map(|mut key| {
                    key[segment..].fill(most);
                    key
                });
                // A NULL key is zeros; a key cut short may stand for other
                // values; a value too long for the key is no key's.
                let cut = by_dynamic
                    && match &given {
                        Val::Str(t) => env.dynamic.as_ref().is_some_and(|d| {
                            d.weights(&t.to_ascii_lowercase()).len() >= key_length
                        }),
                        _ => true,
                    };
                if cut || equal[..segment].iter().all(|&b| b == 0) {
                    return Ok(Some(all));
                }
                if let Val::Str(t) = &given
                    && !by_dynamic
                    && t.len() > segment
                {
                    return Ok(Some(all));
                }
                let excluded: std::collections::HashSet<u32> = order
                    .records_between(Some(&equal), high.as_deref())?
                    .into_iter()
                    .collect();
                return Ok(Some(
                    all.into_iter().filter(|r| !excluded.contains(r)).collect(),
                ));
            }
        };
        let low = match low {
            true => match bound(Side::Low, None)? {
                Some(key) => Some(key),
                None => return Ok(None),
            },
            false => None,
        };
        let high = match high {
            true => match bound(Side::High, None)? {
                Some(key) => Some(key),
                None => return Ok(None),
            },
            false => None,
        };
        order
            .records_between(low.as_deref(), high.as_deref())
            .map(Some)
    }

    /// The length of the part of `order`'s keys the term `compare` is on:
    /// the whole key, its first field's (a key of several fields of an ADI
    /// order), or a leading part's; `None` for a key of several fields of
    /// another order.
    fn term_segment(
        &self,
        order: &Index,
        compare: &tflip_expr::optimize::Compare,
    ) -> Option<usize> {
        let key_length = usize::from(order.tag.key_length);
        Some(match order.key_source(self) {
            seek::KeySource::Field(_) => key_length,
            seek::KeySource::Fields(field) if order.kind == tflip_storage::order::Kind::Adi => {
                tflip_storage::key::field_key_length_wide(field, order.tag.uchar_width)
            }
            seek::KeySource::Fields(_) => return None,
            seek::KeySource::Expression => compare.lead.unwrap_or(key_length).min(key_length),
        })
    }

    /// The records of a `$` term (`name $ 'text'`) as the original engine's
    /// orders find them: those whose key (the part the term is on) is in
    /// the value's text, as bytes (a CICharacter key in lower case, the
    /// value as it is; text padded to the key's length), leaving out NULL
    /// keys of an ADI order. `None` for text keys of a dynamic collation, a
    /// descending order or a value that is not text. [`Table::index_bounds`]
    /// makes the term evaluate so too.
    fn within_records(
        &self,
        order: &Index,
        compare: &tflip_expr::optimize::Compare,
        env: &Env,
    ) -> Result<Option<Vec<u32>>, Error> {
        use tflip_storage::order::Kind;
        let Some(tflip_expr::Val::Str(value)) = tflip_expr::eval_constant(&compare.value, env)
        else {
            return Ok(None);
        };
        if !order.tag.collation.is_empty() || order.tag.descending {
            return Ok(None);
        }
        let Some(segment) = self.term_segment(order, compare) else {
            return Ok(None);
        };
        // An ADI order's text keys under the static collation are weights.
        let collations = self.session_settings.collations();
        let value = match collations.ansi() {
            Some(ansi) if order.kind == Kind::Adi => ansi.key(&value),
            _ => value,
        };
        let mut records = Vec::new();
        for (key, record) in order.keys_between(None, None)? {
            let part = &key[..segment.min(key.len())];
            let null = order.kind == Kind::Adi && part.iter().all(|&b| b == 0);
            if !null && !part.is_empty() && value.windows(part.len()).any(|w| w == part) {
                records.push(record);
            }
        }
        Ok(Some(records))
    }

    /// The records of an `EMPTY()` or `ISNULL()` term (alone, or compared
    /// with a logical value): for `.T.`, those whose key (the part the term
    /// is on) is an empty record's (NULL in an ADT table, blank in a DBF
    /// table), or, in a DBF table, the key of zero or `.F.` in the field; of
    /// `ISNULL()` in a Visual FoxPro table, a NULL key. For `.F.`, the others
    /// but NULL keys, as the original engine's orders find them
    /// (`fixtures/write/aofs_rest`). `None` where those keys are not built.
    fn empty_records(
        &self,
        order: &Index,
        compare: &tflip_expr::optimize::Compare,
        env: &Env,
    ) -> Result<Option<Vec<u32>>, Error> {
        use tflip_expr::optimize::Comparison;
        let Some(segment) = self.term_segment(order, compare) else {
            return Ok(None);
        };
        let cut = |key: Vec<u8>| key[..segment.min(key.len())].to_vec();
        let key_of = |record: Vec<u8>| match self.key_of_bytes(order, record) {
            Ok(Some(key)) => Ok(Some(cut(key))),
            Ok(None) | Err(Error::NotImplemented | Error::KeyOverflow) => Ok(None),
            Err(error) => Err(error),
        };
        let Some(blank) = key_of(self.null_record.clone())? else {
            return Ok(None);
        };
        let mut empty = std::collections::HashSet::from([blank.clone()]);
        if self.format() != Format::Adt
            && let seek::KeySource::Field(field) | seek::KeySource::Fields(field) =
                order.key_source(self)
            && let Some(number) = self
                .fields()
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&field.name))
        {
            for value in [Value::Integer(0), Value::Double(0.0), Value::Logical(false)] {
                if let Ok(Some(key)) = self.value_key(order, number, &value) {
                    empty.insert(cut(key));
                }
            }
        }
        // NULL keys: an ADT table's empty ones; a Visual FoxPro table's
        // those of a record of NULL values, where they are not the blank
        // ones (a field that takes no NULL).
        let nulls = match self.format() {
            Format::Adt => empty.clone(),
            Format::Dbf(tflip_storage::dbf::Flavour::Visual) => {
                let mut record = self.null_record.clone();
                for field in self.fields() {
                    let _ = field.encode(&Value::Null, &mut record);
                }
                match key_of(record)? {
                    Some(key) if key != blank => std::collections::HashSet::from([key]),
                    Some(_) => std::collections::HashSet::new(),
                    None => return Ok(None),
                }
            }
            _ => std::collections::HashSet::new(),
        };
        // A FoxPro table's `ISNULL()` finds the empty keys (see
        // `docs/ADS_BUGS.md`).
        let visual = self.format() == Format::Dbf(tflip_storage::dbf::Flavour::Visual);
        let sought = match compare.op {
            Comparison::Null if visual => &nulls,
            _ => &empty,
        };
        let holds =
            tflip_expr::eval_constant(&compare.value, env) != Some(tflip_expr::Val::Logical(false));
        let keys = order.keys_between(None, None)?;
        let found = keys.into_iter().filter(|(key, _)| {
            let key = cut(key.clone());
            match holds {
                true => sought.contains(&key),
                false => !sought.contains(&key) && !nulls.contains(&key),
            }
        });
        Ok(Some(found.map(|(_, record)| record).collect()))
    }

    /// The records whose keys the orders that serve `bound`'s comparisons
    /// (`orders`, as it numbers them) hold in its ranges, a superset of
    /// those its expression holds on, whatever text it compares (as SQL
    /// compares it, or the expression engine): `None` where they cannot
    /// tell.
    pub(crate) fn lookup_superset(
        &self,
        bound: &tflip_expr::optimize::Bound,
        orders: &[Arc<Index>],
    ) -> Result<Option<Bits>, Error> {
        use tflip_expr::optimize::Bound;
        Ok(match bound {
            Bound::Exact {
                order: Some(order),
                compare: Some(compare),
                ..
            } => {
                let Some(order) = orders.get(*order) else {
                    return Ok(None);
                };
                // SQL compares text by its bytes, not by a dynamic collation.
                let env = Env {
                    dynamic: None,
                    ..self.env()
                };
                self.term_records_as(order, compare, &env, true)?
                    .map(|records| {
                        let mut bits = Bits::default();
                        records
                            .into_iter()
                            .for_each(|record| bits.set(record, true));
                        bits
                    })
            }
            Bound::Fts(records) => Some(Bits::of(records)),
            Bound::And(a, b) => {
                match (
                    self.lookup_superset(a, orders)?,
                    self.lookup_superset(b, orders)?,
                ) {
                    (Some(a), Some(b)) => Some(a.and(&b)),
                    (Some(found), None) | (None, Some(found)) => Some(found),
                    (None, None) => None,
                }
            }
            Bound::Or(a, b) => {
                match (
                    self.lookup_superset(a, orders)?,
                    self.lookup_superset(b, orders)?,
                ) {
                    (Some(a), Some(b)) => Some(a.or(&b)),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    /// How many records the terms of a plan's detail line find, as the
    /// original engine counts them for an SQL statement: those of its
    /// order's records in the ranges that its terms hold on, compared as
    /// with the exact setting on (or for a `.NOT.` before it, the other
    /// records), and the records its terms hold for (before a negation);
    /// `None` for a line no order finds.
    pub(crate) fn detail_records(
        &self,
        detail: &tflip_expr::optimize::Detail,
        orders: &[Arc<Index>],
    ) -> Result<Option<(u32, Bits)>, Error> {
        let Some(order) = detail.order.and_then(|order| orders.get(order)) else {
            return Ok(None);
        };
        let mut sieve = self.sieve();
        sieve.env = Env {
            exact: true,
            ..self.env()
        };
        let mut state = self.state();
        let mut found = Bits::default();
        let mut candidates: Option<Vec<u32>> = None;
        for (_, compare) in &detail.terms {
            let Some(compare) = compare else { continue };
            if let Some(records) = self.term_records(order, compare, &sieve.env)? {
                candidates = Some(match candidates {
                    Some(before) => before.into_iter().filter(|r| records.contains(r)).collect(),
                    None => records,
                });
            }
        }
        let candidates = candidates.unwrap_or_else(|| (1..=state.slots()).collect());
        for slot in candidates {
            if slot == 0 || slot > state.slots() || !state.active(slot)? {
                continue;
            }
            let mut holds = true;
            for (expr, _) in &detail.terms {
                let bytes = state.read(slot, sieve.record_length)?;
                holds &= state.with_memo(|memo| sieve.holds(expr, bytes, slot, memo))?;
            }
            found.set(slot, holds);
        }
        let mut count = found.count();
        if detail.negated {
            let mut active = 0;
            for slot in 1..=state.slots() {
                active += u32::from(state.active(slot)?);
            }
            count = active - count;
        }
        Ok(Some((count, found)))
    }

    /// The number of records the table's AOF shows, its other filters
    /// aside.
    pub(super) fn aof_count(&self) -> Result<u32, Error> {
        let mut sieve = self.sieve();
        let aof = sieve.filters.aof.clone();
        sieve.filters = Arc::new(Filters {
            aof,
            ..Filters::default()
        });
        let mut state = self.state();
        let mut count = 0;
        for slot in 1..=state.slots() {
            count += u32::from(state.visible(slot, &sieve)?);
        }
        Ok(count)
    }

    /// The table's Advantage Optimized Filter; [`Error::NoFilter`] if it has
    /// none.
    pub fn aof(&self) -> Result<String, Error> {
        let filters = Arc::clone(&lock(&self.filters));
        let aof = filters.aof.as_ref().filter(|aof| !aof.orphaned);
        aof.map(|aof| aof.filter.text.clone())
            .ok_or(Error::NoFilter)
    }

    /// How far the table's AOF is optimized, and the part of it that is not
    /// (`AdsGetAOFOptLevel`); [`Error::NoFilter`] if it has none.
    pub fn aof_level(&self) -> Result<(tflip_expr::optimize::Level, String), Error> {
        let filters = Arc::clone(&lock(&self.filters));
        let aof = filters
            .aof
            .as_ref()
            .filter(|aof| !aof.orphaned)
            .ok_or(Error::NoFilter)?;
        Ok((aof.level, aof.residual.clone()))
    }

    /// Removes the table's Advantage Optimized Filter; [`Error::NoFilter`]
    /// if it has none.
    pub fn clear_aof(&self) -> Result<(), Error> {
        if lock(&self.filters).parked.is_none() {
            self.aof()?;
        }
        self.change_filters(|filters| {
            filters.aof = None;
            filters.parked = None;
        });
        Ok(())
    }

    /// Sets the table's AOF aside, so that it hides no records
    /// (`AdsDeactivateAOF`), or puts it back (`AdsActivateAOF`). Either is
    /// [`Error::NoFilter`] if the table has no AOF, set aside or not.
    pub fn set_aof_active(&self, active: bool) -> Result<(), Error> {
        let mut filters = lock(&self.filters);
        let has = filters.aof.as_ref().is_some_and(|aof| !aof.orphaned);
        if !has && filters.parked.is_none() {
            return Err(Error::NoFilter);
        }
        let filters = Arc::make_mut(&mut filters);
        match active {
            true if filters.parked.is_some() => filters.aof = filters.parked.take(),
            false if has => filters.parked = filters.aof.take(),
            _ => {}
        }
        Ok(())
    }

    /// Resolves the table's AOF anew (`AdsRefreshAOF`), as when it was set,
    /// with the index orders `orders`: its customization is dropped, and it
    /// counts its records again.
    /// Nothing happens to a table without one.
    pub fn refresh_aof(&self, orders: &[Arc<Index>]) -> Result<(), Error> {
        let filters = Arc::clone(&lock(&self.filters));
        let Some(aof) = filters.aof.clone().filter(|aof| !aof.orphaned) else {
            return Ok(());
        };
        self.try_set_aof(&aof.filter.text, aof.options, orders)
    }

    /// Adds `records` to the table's AOF, removes them from it or toggles
    /// them (`AdsCustomizeAOF`), whatever its expression says of them, and
    /// counts its records again; [`Error::NoFilter`] if the table has no
    /// AOF. From then on, writes do not change the records in it. As in the
    /// original engine, a record added to an AOF that is resolved as moves
    /// reach records and not fully optimized is a candidate, which it
    /// evaluates.
    pub fn customize_aof(&self, records: &[u32], how: Customize) -> Result<(), Error> {
        let sieve = self.sieve();
        let aof = sieve
            .filters
            .aof
            .clone()
            .filter(|aof| !aof.orphaned)
            .ok_or(Error::NoFilter)?;
        let mut members = lock(&aof.members);
        if !members.frozen {
            // The records in it now: what its expression says of each one
            // it has evaluated.
            let mut state = self.state();
            let changes = state.changes()?;
            for slot in 1..=state.slots() {
                let outside = match &members.superset {
                    Some((at, superset)) => *at == changes && !superset.get(slot),
                    None => false,
                };
                if outside || members.unresolved.get(slot) || !state.active(slot)? {
                    continue;
                }
                let bytes = state.read(slot, sieve.record_length)?;
                let holds =
                    state.with_memo(|memo| sieve.holds(&aof.filter.expr, bytes, slot, memo))?;
                members.inside.set(slot, holds);
            }
            members.frozen = true;
        }
        for &record in records {
            let member = members.inside.get(record) || members.unresolved.get(record);
            let add = match how {
                Customize::Add => true,
                Customize::Remove => false,
                Customize::Toggle => !member,
            };
            if !add {
                members.inside.set(record, false);
                members.unresolved.set(record, false);
            } else if aof.exact() {
                members.inside.set(record, true);
            } else if !members.inside.get(record) {
                members.unresolved.set(record, true);
            }
        }
        drop(members);
        let count = self.aof_count()?;
        lock(&aof.members).count = Some(count);
        Ok(())
    }

    /// Whether `record` is in the table's Advantage Optimized Filter
    /// (`AdsIsRecordInAOF`); [`Error::NoFilter`] if it has none. As the
    /// original engine answers, a candidate the AOF has not evaluated yet
    /// is. A record that does not exist, or was deleted when the AOF was
    /// set, is not; one deleted since is as it was.
    pub fn is_in_aof(&self, record: u32) -> Result<bool, Error> {
        let mut sieve = self.sieve();
        // An AOF set aside still answers.
        if let Some(parked) = &sieve.filters.parked {
            let filters = Filters {
                aof: Some(parked.clone()),
                ..(*sieve.filters).clone()
            };
            sieve.filters = Arc::new(filters);
        }
        if sieve.filters.aof.as_ref().is_none_or(|aof| aof.orphaned) {
            return Err(Error::NoFilter);
        }
        let mut state = self.state();
        if record == 0 || record > state.slots() {
            return Ok(false);
        }
        if !state.active(record)? {
            let aof = sieve.filters.aof.as_ref().ok_or(Error::NoFilter)?;
            let gone = lock(&aof.members).gone.get(record);
            if gone || !state.exists_deleted(record)? {
                return Ok(false);
            }
        }
        let bytes = state.read(record, sieve.record_length)?;
        state.with_memo(|memo| sieve.in_aof(bytes, record, memo))
    }

    /// The full-text rules of each of the table's fields for `CONTAINS`
    /// outside SQL: those of the field's full-text order among `orders` (the
    /// orders open on the table) or its index files, else every word
    /// counting, as the original searches a field with none
    /// (`fixtures/expr/contains.json`).
    fn contains_rules(&self, orders: &[Arc<Index>]) -> Vec<tflip_sql::fts::Rules> {
        let mut rules: Vec<tflip_sql::fts::Rules> = crate::index::fts_rules(self)
            .into_iter()
            .map(|rules| rules.unwrap_or_else(tflip_sql::fts::Rules::non_indexed))
            .collect();
        for order in orders.iter().filter(|order| order.tag.full_text()) {
            if let (Some(place), Ok(found)) = (self.full_text_place(order), order.fts_rules())
                && let Some(slot) = rules.get_mut(place)
            {
                *slot = (*found).clone();
            }
        }
        rules
    }

    /// The place of the field a full-text order is on: named by its key, or
    /// as `F<n>` (which an order of a DBF table made on its handle says).
    fn full_text_place(&self, order: &Index) -> Option<usize> {
        let field = match order.key_source(self) {
            crate::seek::KeySource::Field(field) => field,
            _ => {
                let number = order
                    .tag
                    .expression
                    .trim()
                    .strip_prefix('F')?
                    .parse()
                    .ok()?;
                self.key_field(number).ok()?
            }
        };
        self.fields()
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&field.name))
    }

    /// What answers the `CONTAINS` of an AOF's `expr`, where it has one. As
    /// in the original engine, a condition on a field with a full-text order
    /// is parsed when the AOF is set, and one the order cannot search is
    /// refused (7103); others are parsed as records are read.
    fn contains_hook(
        &self,
        expr: &tflip_expr::Expr,
        orders: &[Arc<Index>],
    ) -> Result<Option<tflip_expr::Contains>, Error> {
        if !expr.uses_contains() {
            return Ok(None);
        }
        let rules = self.contains_rules(orders);
        let indexed: Vec<bool> = {
            let mut indexed = vec![false; rules.len()];
            for (place, found) in crate::index::fts_rules(self).iter().enumerate() {
                indexed[place] = found.is_some();
            }
            for order in orders.iter().filter(|order| order.tag.full_text()) {
                if let Some(place) = self.full_text_place(order)
                    && let Some(slot) = indexed.get_mut(place)
                {
                    *slot = true;
                }
            }
            indexed
        };
        for (place, condition) in expr.contains_terms() {
            if indexed.get(place) == Some(&true) {
                let condition = tflip_storage::cp1252::decode(&condition);
                tflip_sql::fts::parse(&condition, &rules[place])
                    .map_err(|error| Error::Evaluation(tflip_expr::Runtime::FullText(error.0)))?;
            }
        }
        Ok(Some(contains_with(rules)))
    }

    /// Sets the table's record filter (`AdsSetFilter`), replacing any it
    /// had. It is kept as given. An expression that is not valid is
    /// [`Error::Expression`], an empty one [`Error::InvalidExpression`]. As
    /// in the original engine, the expression need not be logical (see
    /// [`crate::filter`]), and the table does not move.
    pub fn set_filter(&self, expression: &str, orders: &[Arc<Index>]) -> Result<(), Error> {
        let expr = self.compile(expression)?;
        // A filter's `CONTAINS` conditions are parsed as records are read.
        let contains = match expr.uses_contains() {
            true => Some(contains_with(self.contains_rules(orders))),
            false => None,
        };
        let filter = Filter {
            text: expression.to_owned(),
            expr: Arc::new(expr),
            contains,
        };
        self.change_filters(|filters| filters.filter = Some(filter));
        Ok(())
    }

    /// The table's record filter; [`Error::NoFilter`] if it has none.
    pub fn filter(&self) -> Result<String, Error> {
        let filters = Arc::clone(&lock(&self.filters));
        filters
            .filter
            .as_ref()
            .map(|filter| filter.text.clone())
            .ok_or(Error::NoFilter)
    }

    /// Removes the table's record filter; [`Error::NoFilter`] if it has
    /// none.
    pub fn clear_filter(&self) -> Result<(), Error> {
        self.filter()?;
        self.change_filters(|filters| filters.filter = None);
        Ok(())
    }

    pub(super) fn change_filters(&self, change: impl FnOnce(&mut Filters)) {
        let mut filters = lock(&self.filters);
        change(Arc::make_mut(&mut filters));
    }

    /// Moves to the first visible record, from the top (`forward`) or from
    /// the bottom, for which `expression` holds (`AdsLocate`), and returns
    /// whether there is one; if not, the table goes to EOF (or BOF).
    pub fn locate(&self, expression: &str, forward: bool) -> Result<bool, Error> {
        let expr = self.compile(expression)?;
        let mut state = self.state();
        state.located = Some((expression.to_owned(), forward));
        let start = if forward { 0 } else { state.slots() + 1 };
        drop(state);
        self.locate_from(&expr, start, forward)
    }

    /// Goes on from the current record with the last [`Table::locate`]
    /// (`AdsContinue`): to the next visible record in its direction for
    /// which its condition holds, or to EOF (or BOF) if there is none.
    /// Without one it is [`Error::NoLocate`].
    pub fn continue_locate(&self) -> Result<bool, Error> {
        let state = self.state();
        let (expression, forward) = state.located.clone().ok_or(Error::NoLocate)?;
        let start = match state.position {
            Position {
                record: Some(record),
                ..
            } => record,
            Position { eof: true, .. } if forward => state.slots() + 1,
            _ if forward => 0,
            _ => state.slots() + 1,
        };
        drop(state);
        let expr = self.compile(&expression)?;
        self.locate_from(&expr, start, forward)
    }

    /// Moves to the first visible record after slot `slot` (`forward`) or
    /// before it for which `expr` holds; or to EOF (BOF) if there is none.
    pub(super) fn locate_from(
        &self,
        expr: &Expr,
        mut slot: u32,
        forward: bool,
    ) -> Result<bool, Error> {
        let sieve = self.sieve();
        let env = self.env();
        let mut state = self.state();
        while let Some(next) = state.next_visible(slot, forward, &sieve)? {
            slot = next;
            let bytes = state.read(slot, sieve.record_length)?;
            let holds = state.with_memo(|memo| {
                let mut row = filter::Record {
                    fields: self.fields(),
                    bytes,
                    memo,
                    number: slot,
                };
                expr.holds(&mut row, &env).unwrap_or(false)
            });
            if holds {
                state.land(Some(slot), forward, &sieve)?;
                state.found = true;
                return Ok(true);
            }
        }
        state.land(None, forward, &sieve)?;
        Ok(false)
    }

    /// Makes the terms of `bound` that its orders (`orders`, as it numbers
    /// them) find otherwise than the expression engine evaluates them
    /// evaluate as the original engine's orders find them:
    ///
    /// - `YEAR()` terms, by the year the value's whole part is: an order on
    ///   the date finds the dates from its first day to its last (for a year
    ///   it cannot make a date of, a CDX order the empty date, an ADI order
    ///   none); an order on `DTOS()` of it the keys whose first four
    ///   characters compare with the year's digits; these become plain
    ///   comparisons;
    /// - `$` terms, by the key's bytes (see [`Table::within_records`]): the
    ///   key's text padded to its width (`PADR()`, which also compares a
    ///   CICharacter key in lower case with the value as it is; but not
    ///   under a static collation, which folds such keys otherwise);
    /// - `ISNULL()` through a FoxPro order, whose keys hold no NULL: the
    ///   blank keys, as `EMPTY()` (and `= .F.` the others);
    /// - `EMPTY() = .F.` through a Visual FoxPro order: the keys neither
    ///   blank nor NULL;
    /// - `=` and `<>` of a leading part of a FoxPro key of several fields
    ///   (`a` of `A+C`) with text longer than the part: the key's leading
    ///   text, as a comparison of the key's expression.
    ///
    /// Returns the AOF `expression` as it is evaluated then, with those terms
    /// replaced (`None` if it has none), so that records show as the
    /// original engine's orders find them.
    pub(super) fn index_bounds(
        &self,
        expression: &str,
        bound: &mut tflip_expr::optimize::Bound,
        orders: &[Arc<Index>],
    ) -> Result<Option<String>, Error> {
        let mut replaced = Vec::new();
        let mut chars: Vec<char> = expression.chars().collect();
        self.index_terms(bound, orders, &self.env(), &chars, &mut replaced)?;
        if replaced.is_empty() {
            return Ok(None);
        }
        replaced.sort_by_key(|(term, _)| std::cmp::Reverse(term.start));
        for (term, text) in replaced {
            if term.end > chars.len() {
                return Ok(None);
            }
            chars.splice(term, format!("({text})").chars());
        }
        Ok(Some(chars.into_iter().collect()))
    }

    fn index_terms(
        &self,
        bound: &mut tflip_expr::optimize::Bound,
        orders: &[Arc<Index>],
        env: &Env,
        chars: &[char],
        replaced: &mut Vec<(std::ops::Range<usize>, String)>,
    ) -> Result<(), Error> {
        use tflip_expr::optimize::{Bound, Compare, Comparison};
        use tflip_expr::{Val, eval_constant};
        match bound {
            Bound::And(a, b) | Bound::Or(a, b) => {
                self.index_terms(a, orders, env, chars, replaced)?;
                self.index_terms(b, orders, env, chars, replaced)
            }
            Bound::Not(a) => self.index_terms(a, orders, env, chars, replaced),
            Bound::All | Bound::Fts(_) => Ok(()),
            Bound::Exact {
                order: Some(number),
                compare: Some(compare),
                expr,
            } if compare.year.is_none() => {
                let Some(order) = orders.get(*number) else {
                    return Ok(());
                };
                if let Some(text) = self.found_term(order, compare, env, chars)? {
                    *expr = self.compile(&text)?;
                    replaced.extend(compare.term.clone().map(|term| (term, text)));
                }
                Ok(())
            }
            Bound::Exact {
                order: Some(number),
                compare: Some(compare),
                ..
            } => {
                let number = *number;
                let (Some(year), Some(order)) = (&compare.year, orders.get(number)) else {
                    return Ok(());
                };
                let Some(Val::Number(n)) = eval_constant(&compare.value, env) else {
                    return Ok(());
                };
                // The whole part, kept to what four digits and a sign hold.
                let y = n.trunc().clamp(-99_999.0, 99_999.0) as i32;
                let date = year.date.clone();
                let term = year.term.clone();
                let exact = |op: Comparison, value: &str| -> Result<Bound, Error> {
                    let sign = if op == Comparison::Ge { ">=" } else { "<=" };
                    Ok(Bound::Exact {
                        expr: self.compile(&format!("{date}{sign}{value}"))?,
                        order: Some(number),
                        compare: Some(Compare {
                            op,
                            value: self.compile(value)?,
                            year: None,
                            lead: None,
                            term: None,
                            left: None,
                        }),
                    })
                };
                let (text, plain) = match (year.dtos, compare.op) {
                    (false, Comparison::Eq) if (1..=9999).contains(&y) => {
                        let first = format!("STOD('{y:04}0101')");
                        let last = format!("STOD('{y:04}1231')");
                        let plain = Bound::And(
                            Box::new(exact(Comparison::Ge, &first)?),
                            Box::new(exact(Comparison::Le, &last)?),
                        );
                        (format!("{date}>={first}.AND.{date}<={last}"), plain)
                    }
                    (false, Comparison::Eq) => {
                        let text = match order.kind {
                            tflip_storage::order::Kind::Cdx => format!("DTOS({date})=='        '"),
                            _ => ".F.".to_owned(),
                        };
                        let expr = self.compile(&text)?;
                        (
                            text,
                            Bound::Exact {
                                expr,
                                order: Some(number),
                                compare: None,
                            },
                        )
                    }
                    (false, _) => return Ok(()),
                    (true, op) => {
                        let (sign, op) = match op {
                            Comparison::Eq | Comparison::Exactly => ("=", Comparison::Eq),
                            Comparison::Ne => ("<>", op),
                            Comparison::Lt => ("<", op),
                            Comparison::Le => ("<=", op),
                            Comparison::Gt => (">", op),
                            Comparison::Ge => (">=", op),
                            Comparison::Within | Comparison::Empty | Comparison::Null => {
                                return Ok(());
                            }
                        };
                        let digits = format!("'{y:04}'");
                        let text = format!("LEFT(DTOS({date}),4){sign}{digits}");
                        let value = self.compile(&digits)?;
                        let compare = Compare {
                            op,
                            value,
                            year: None,
                            lead: None,
                            term: None,
                            left: None,
                        };
                        let expr = self.compile(&text)?;
                        (
                            text,
                            Bound::Exact {
                                expr,
                                order: Some(number),
                                compare: Some(compare),
                            },
                        )
                    }
                };
                replaced.push((term, text));
                *bound = plain;
                Ok(())
            }
            Bound::Exact { .. } => Ok(()),
        }
    }

    /// The text a term `compare` that `order` finds evaluates as when the
    /// original engine's order finds it otherwise than the expression
    /// engine evaluates it (see [`Table::index_bounds`]); `None` if it does
    /// not. `chars` are the expression's.
    fn found_term(
        &self,
        order: &Index,
        compare: &tflip_expr::optimize::Compare,
        env: &Env,
        chars: &[char],
    ) -> Result<Option<String>, Error> {
        use tflip_expr::optimize::Comparison;
        use tflip_storage::order::Kind;
        let Some(term) = compare.term.clone().filter(|term| term.end <= chars.len()) else {
            return Ok(None);
        };
        let text = |range: std::ops::Range<usize>| chars[range].iter().collect::<String>();
        let left = compare
            .left
            .clone()
            .filter(|left| term.start <= left.start && left.end <= term.end);
        Ok(match compare.op {
            Comparison::Within if order.kind != Kind::Ntx && order.tag.collation.is_empty() => {
                let Some(left) = left else { return Ok(None) };
                let width = match self.compile(&text(left.clone()))?.ty() {
                    // CICharacter text folded by a static collation, as its
                    // keys are, is not text expressions see (they fold ASCII
                    // letters alone): the term evaluates as it is.
                    tflip_expr::Type::String { ci: true, .. } if env.case_fold.is_some() => {
                        return Ok(None);
                    }
                    tflip_expr::Type::String { width, .. } => width,
                    _ => return Ok(None),
                };
                Some(format!(
                    "{}PADR({},{width}){}",
                    text(term.start..left.start),
                    text(left.clone()),
                    text(left.end..term.end)
                ))
            }
            Comparison::Null
                if order.kind == Kind::Cdx
                    && self.format() == Format::Dbf(tflip_storage::dbf::Flavour::FoxPro) =>
            {
                let written = text(term.clone());
                let trimmed = written.trim_start();
                let at = written.len() - trimmed.len();
                match trimmed.get(..6) {
                    Some(name) if name.eq_ignore_ascii_case("ISNULL") => {
                        Some(format!("{}EMPTY{}", &written[..at], &trimmed[6..]))
                    }
                    _ => None,
                }
            }
            // A Visual FoxPro order's `EMPTY() = .F.` leaves NULL out.
            Comparison::Empty
                if self.format() == Format::Dbf(tflip_storage::dbf::Flavour::Visual)
                    && tflip_expr::eval_constant(&compare.value, env)
                        == Some(tflip_expr::Val::Logical(false)) =>
            {
                let Some(left) = left else { return Ok(None) };
                let written = text(left);
                let trimmed = written.trim_start();
                match trimmed.get(..5) {
                    Some(name) if name.eq_ignore_ascii_case("EMPTY") => Some(format!(
                        "{} .AND. .NOT. ISNULL{}",
                        text(term.clone()),
                        &trimmed[5..]
                    )),
                    _ => None,
                }
            }
            Comparison::Eq | Comparison::Ne if let Some(lead) = compare.lead => {
                let Some(left) = left else { return Ok(None) };
                match tflip_expr::eval_constant(&compare.value, env) {
                    Some(tflip_expr::Val::Str(value)) if value.len() > lead => Some(format!(
                        "{}{}{}",
                        text(term.start..left.start),
                        order.expression(),
                        text(left.end..term.end)
                    )),
                    _ => None,
                }
            }
            _ => None,
        })
    }
}

/// The value `field` holds that is `v` itself; `None` if it holds none (a
/// number it would round).
fn exact_value(field: &Field, v: &tflip_sql::V) -> Option<Value> {
    match field.field_type {
        FieldType::Double | FieldType::CurDouble => {
            let d = v.as_f64().filter(|_| !matches!(v, tflip_sql::V::Bool(_)))?;
            Some(Value::Double(d))
        }
        _ => {
            let low = crate::statement::bound_value(field, v, false)?;
            let high = crate::statement::bound_value(field, v, true)?;
            (low == high).then_some(low)
        }
    }
}

/// The `CONTAINS` of filters searching each field's text with its `rules`.
fn contains_with(rules: Vec<tflip_sql::fts::Rules>) -> tflip_expr::Contains {
    tflip_expr::Contains(Arc::new(move |field, text, condition| {
        let default = tflip_sql::fts::Rules::non_indexed();
        let rules = field.and_then(|field| rules.get(field)).unwrap_or(&default);
        let text = tflip_storage::cp1252::decode(text);
        let condition = tflip_storage::cp1252::decode(condition);
        tflip_sql::fts::search(&condition, &text, rules).map_err(|error| error.0)
    }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::Session;

    /// The records the orders find for terms they serve beyond plain
    /// comparisons (`fixtures/write/aofs_lookup`, whose replay checks the
    /// records AOFs show): `<>`, the first field of a key of several fields,
    /// and `YEAR()` through an order on the date or on `DTOS()` of it. They
    /// are found in the orders, not by evaluating every record.
    #[test]
    fn orders_find_more_terms() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/aofs_lookup/before");
        let session = Session::new();
        let settings = |table_type| TableSettings {
            table_type,
            collation: "ansi".to_owned(),
            options: 4,
            ..TableSettings::default()
        };
        let cases: [(&str, TableType, &str, &[u32]); 12] = [
            (
                "p.adt",
                TableType::Adt,
                "id <> 3",
                &[1, 2, 4, 5, 6, 7, 9, 10],
            ),
            (
                "p.adt",
                TableType::Adt,
                "d <> 0.3",
                &[1, 2, 3, 4, 6, 7, 8, 9, 10],
            ),
            (
                "p.adt",
                TableType::Adt,
                "name <> 'Bob'",
                &[2, 3, 4, 5, 6, 8, 9, 10],
            ),
            (
                "p.adt",
                TableType::Adt,
                "n <> 1.505",
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            ),
            ("p.adt", TableType::Adt, "YEAR(born) = 1990.5", &[1, 3, 5]),
            ("m.adt", TableType::Adt, "a = 'ab'", &[1, 2, 3]),
            ("m.adt", TableType::Adt, "a <> 'ab'", &[3, 4, 5, 6, 7]),
            ("m.adt", TableType::Adt, "b > 1", &[1, 2, 3, 4, 5, 7]),
            ("m.adt", TableType::Adt, "YEAR(c) = 2000", &[1, 2, 6, 7]),
            ("s.adt", TableType::Adt, "YEAR(born) < 1990", &[1, 2, 3]),
            ("c.dbf", TableType::Cdx, "YEAR(born) >= 1990", &[1, 3, 4, 6]),
            ("c.dbf", TableType::Cdx, "id <> 2", &[1, 3, 4, 5, 6]),
        ];
        for (name, table_type, expression, expected) in cases {
            let path = dir.join(name);
            let table = open(&session, 0, path.to_str().unwrap(), settings(table_type)).unwrap();
            let opened = get(&session, table).unwrap();
            let orders = index::open_on(&session, table);
            let (mut optimized, serving) = opened.optimize_with(expression, &orders, &[]).unwrap();
            opened
                .index_bounds(expression, &mut optimized.bound, &serving)
                .unwrap();
            let found = opened.lookup_superset(&optimized.bound, &serving).unwrap();
            let found: Option<Vec<u32>> = found.map(|bits| bits.records().collect());
            assert_eq!(found.as_deref(), Some(expected), "{name}: {expression}");
            close(&session, table).unwrap();
        }
    }

    /// The records the orders find, sorted, for each AOF of `cases` (a
    /// single term) on the tables in `dir`, as setting the AOF finds them.
    fn found_by_orders(session: &Session, dir: &Path, cases: &[(&str, TableType, &str, &[u32])]) {
        use tflip_expr::optimize::Bound;
        for &(name, table_type, expression, expected) in cases {
            let settings = TableSettings {
                table_type,
                collation: "ansi".to_owned(),
                options: 4,
                ..TableSettings::default()
            };
            let path = dir.join(name);
            let table = open(session, 0, path.to_str().unwrap(), settings).unwrap();
            let opened = get(session, table).unwrap();
            let orders = index::open_on(session, table);
            let (mut optimized, serving) = opened.optimize_with(expression, &orders, &[]).unwrap();
            opened
                .index_bounds(expression, &mut optimized.bound, &serving)
                .unwrap();
            let Bound::Exact {
                order: Some(order),
                compare: Some(compare),
                ..
            } = &optimized.bound
            else {
                panic!("{name}: {expression}: {:?}", optimized.bound);
            };
            let found = opened
                .term_records(&serving[*order], compare, &opened.env())
                .unwrap();
            let found = found.map(|mut records| {
                records.sort();
                records
            });
            assert_eq!(found.as_deref(), Some(expected), "{name}: {expression}");
            close(session, table).unwrap();
        }
    }

    /// The records the orders find for `EMPTY()` and `$` terms and for terms
    /// on the leading part of a FoxPro key of several fields
    /// (`fixtures/write/aofs_more`, whose replay checks the records AOFs
    /// show): looked up in the orders, not by evaluating every record, as
    /// the original engine finds them (NULL keys of an ADT table, blank ones
    /// of a FoxPro table, `ISNULL()` among them; `$` by the keys' bytes).
    #[test]
    fn orders_find_empty_within_and_parts() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/aofs_more/before");
        let session = Session::new();
        let (adt, cdx) = (TableType::Adt, TableType::Cdx);
        found_by_orders(
            &session,
            &dir,
            &[
                ("p.adt", adt, "EMPTY(name)", &[3]),
                ("p.adt", adt, "EMPTY(UPPER(name))", &[3]),
                ("p.adt", adt, "ISNULL(born) = .T.", &[3]),
                ("p.adt", adt, "name $ 'Bob        x'", &[1, 6]),
                ("p.adt", adt, "ciname $ 'xbob       '", &[1, 7]),
                ("p.adt", adt, "ciname $ 'xBOB       '", &[]),
                ("p.adt", adt, "vc $ 'xBob       '", &[1]),
                ("p.adt", adt, "vc $ 'Bob'", &[]),
                ("m.adt", adt, "EMPTY(a)", &[3]),
                ("m.adt", adt, "a $ 'xabc  '", &[5]),
                ("c.dbf", cdx, "EMPTY(name)", &[2]),
                ("c.dbf", cdx, "EMPTY(id)", &[3]),
                ("c.dbf", cdx, "EMPTY(born)", &[2]),
                ("c.dbf", cdx, "EMPTY(active)", &[2, 4, 6]),
                ("c.dbf", cdx, "EMPTY(amount)", &[2]),
                ("c.dbf", cdx, "ISNULL(name)", &[2]),
                ("c.dbf", cdx, "name $ ' Bob       '", &[1, 3]),
                ("cm.dbf", cdx, "a = 'ab'", &[1, 2, 3, 8]),
                ("cm.dbf", cdx, "a + c = 'ab   x'", &[1]),
                ("cm.dbf", cdx, "c = 'x'", &[1, 3]),
                ("cm.dbf", cdx, "EMPTY(c)", &[6, 8]),
                ("cm.dbf", cdx, "EMPTY(a)", &[6]),
                ("cm.dbf", cdx, "a $ 'xab   '", &[1, 2, 8]),
                ("cm.dbf", cdx, "DTOS(d) = '2000'", &[1, 2, 6, 7, 8]),
            ],
        );
    }

    /// Text under the static collation `ANSI_CHAR_SET=German`
    /// (`fixtures/write/aofs_static`) is looked up in ADI and CDX orders by
    /// its weights (a bound takes in the keys that start with the value).
    /// Needs the collation files, as the German collation does.
    #[test]
    fn orders_find_text_under_a_static_collation() {
        let dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let Some(collations) = dir.filter(|dir| dir.join("ansi.chr").exists()) else {
            return;
        };
        let session = Session::new();
        crate::settings::set_collations(
            &session,
            crate::collations::Collations::new(&collations, "German", ""),
        );
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/aofs_static/before");
        let (adt, cdx) = (TableType::Adt, TableType::Cdx);
        found_by_orders(
            &session,
            &dir,
            &[
                ("g.adt", adt, "name = 'apfel'", &[1]),
                ("g.adt", adt, "name = 'ä'", &[7]),
                ("g.adt", adt, "name > 'ss'", &[5, 6, 9]),
                (
                    "g.adt",
                    adt,
                    "name <> 'apfel'",
                    &[2, 3, 4, 5, 6, 7, 8, 9, 10],
                ),
                ("g.adt", adt, "UPPER(name) = 'ÄPFEL'", &[3, 7]),
                ("c.dbf", cdx, "name = 'Äpfel'", &[3]),
                ("c.dbf", cdx, "name < 'ä'", &[1, 2, 7, 8]),
                ("c.dbf", cdx, "name >= 'Äpfel'", &[3, 4, 5, 6, 9]),
                ("c.dbf", cdx, "UPPER(name) = 'APFEL'", &[1, 2]),
            ],
        );
    }

    /// The records the orders find for `EMPTY()` and `ISNULL()` of Visual
    /// FoxPro keys (blank ones and NULL ones), and for `EMPTY() = .F.`
    /// (`fixtures/write/aofs_rest`): looked up in the orders, leaving NULL
    /// keys out of `= .F.` as the original engine does.
    #[test]
    fn orders_find_empty_and_null_keys() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/aofs_rest/before");
        let session = Session::new();
        let (adt, cdx, vfp) = (TableType::Adt, TableType::Cdx, TableType::Vfp);
        found_by_orders(
            &session,
            &dir,
            &[
                ("v.dbf", vfp, "EMPTY(name)", &[2]),
                ("v.dbf", vfp, "EMPTY(d)", &[2, 5]),
                ("v.dbf", vfp, "EMPTY(nn)", &[2, 3]),
                ("v.dbf", vfp, "ISNULL(name)", &[3]),
                ("v.dbf", vfp, "ISNULL(nn)", &[]),
                ("v.dbf", vfp, "ISNULL(UPPER(name))", &[3]),
                ("v.dbf", vfp, "EMPTY(name) = .F.", &[1, 4, 5, 6]),
                ("v.dbf", vfp, "ISNULL(born) = .F.", &[1, 2, 4, 5, 6]),
                ("c.dbf", cdx, "EMPTY(name) <> .T.", &[1, 3, 4, 5]),
                ("c.dbf", cdx, "ISNULL(id) = .F.", &[1, 2, 4, 5]),
                ("cm.dbf", cdx, "EMPTY(a) = .F.", &[1, 3]),
                ("p.adt", adt, "EMPTY(id) = .F.", &[1, 2, 4]),
                ("m.adt", adt, "EMPTY(a) = .F.", &[1, 2, 4]),
            ],
        );
    }

    /// Under `ANSI_CHAR_SET=German` (`fixtures/write/aofs_static_keys`),
    /// CICharacter text of an ADI order on the field is looked up by its
    /// keys (folded by the collation, then weighed), and numbers and dates of
    /// CDX orders by their bytes, which the collation does not weigh.
    #[test]
    fn orders_find_ci_text_and_cdx_numbers_under_a_static_collation() {
        let dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let Some(collations) = dir.filter(|dir| dir.join("ansi.chr").exists()) else {
            return;
        };
        let session = Session::new();
        crate::settings::set_collations(
            &session,
            crate::collations::Collations::new(&collations, "German", ""),
        );
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/write/aofs_static_keys/before");
        let (adt, cdx) = (TableType::Adt, TableType::Cdx);
        found_by_orders(
            &session,
            &dir,
            &[
                ("g.adt", adt, "ci = 'äPFEL'", &[3, 7]),
                ("g.adt", adt, "ci = 'Öl'", &[12, 13]),
                ("g.adt", adt, "ci > 'ss'", &[5, 6, 9]),
                ("c.dbf", cdx, "id >= -1", &[1, 3, 4, 5, 6, 7, 8]),
                ("c.dbf", cdx, "amount > 0", &[1, 3, 4, 5, 7, 9]),
                ("c.dbf", cdx, "d < 1", &[2, 3, 6, 8, 9]),
                ("c.dbf", cdx, "born > STOD('19900105')", &[1, 3, 5, 7, 8]),
                ("c.dbf", cdx, "id + 1 = 3", &[5]),
            ],
        );
    }
}
