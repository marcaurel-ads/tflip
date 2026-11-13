//! Expressions on a table: evaluated on its current record (`AdsEval*Expr`,
//! `AdsIsExprValid`), searched for (`AdsLocate`, `AdsContinue`), and
//! converted between field names and numbers (`AdsExpressionLongToShort`,
//! `AdsExpressionShortToLong`). See the `tflip-expr` crate for the language.
//!
//! The `AdsEval*Expr` functions are obsolete in the original engine but
//! still there, and answer as it does: first whether the expression
//! compiles (35xx codes), then whether its type suits the function
//! (`AE_INVALID_EXPRESSION`), then whether the table is on a record
//! (`AE_NO_CURRENT_RECORD`), then the value. A NULL value is false, 0, or
//! as many zero bytes as the type is wide.

use tflip_engine::filter::Evaluation;
use tflip_engine::filter::{Type, Val, number};
use tflip_engine::handle::HandleKind;

use crate::backend::api;
use crate::ffi::{c_str, write_name, write_units};
use crate::filter::{Encoding, Expression, table_of};
use crate::last_error::{fail, fail_handle, null_param};
use crate::{
    ADS_FALSE, ADS_TRUE, AE_INVALID_EXPRESSION, AE_INVALID_HANDLE, AE_INVALID_POINTER,
    AE_INVALID_TABLE_HANDLE, AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16, Unsigned32,
    VALIDITY_ERRORS, ansi, error_code, write_cut_at,
};

/// The code of `AdsEvalNumericExpr` for a field's value that does not fit
/// the field's display format (a Double of 1e300).
pub const NUMBER_DOES_NOT_FIT: Unsigned32 = 4504;

/// Evaluates `expr` on the current record of `table`, after checking the
/// arguments: the table, and `expr` and `out` given (`None` for a null
/// pointer). `out` is handed back with the evaluation.
fn evaluate<O>(
    table: AdsHandle,
    expr: Option<Expression>,
    out: Option<O>,
    param: u32,
) -> Result<(Evaluation, O), Unsigned32> {
    let backend = table_of(table)?;
    let Some(text) = expr else {
        return Err(null_param(2));
    };
    let Some(out) = out else {
        return Err(null_param(param));
    };
    let evaluation = backend.eval(table, text.engine).map_err(error_code)?;
    Ok((evaluation, out))
}

/// The value of an evaluation whose type `suits` the function; else
/// `AE_INVALID_EXPRESSION`.
fn value(evaluation: Evaluation, suits: impl Fn(Type) -> bool) -> Result<(Type, Val), Unsigned32> {
    if !suits(evaluation.ty) {
        return Err(fail(AE_INVALID_EXPRESSION));
    }
    evaluation
        .value
        .map(|value| (evaluation.ty, value))
        .map_err(error_code)
}

/// `AdsEvalLogicalExpr` and `AdsEvalLogicalExprW`.
fn eval_logical(
    table: AdsHandle,
    expr: Option<Expression>,
    result: Option<&mut Unsigned16>,
) -> Unsigned32 {
    let (evaluation, result) = match evaluate(table, expr, result, 3) {
        Ok(evaluated) => evaluated,
        Err(code) => return code,
    };
    match value(evaluation, |ty| ty == Type::Logical) {
        Ok((_, value)) => {
            let holds = value == Val::Logical(true);
            *result = if holds { ADS_TRUE } else { ADS_FALSE };
            AE_SUCCESS
        }
        Err(code) => code,
    }
}

/// `AdsEvalLogicalExpr`: the value of a logical expression on the current
/// record, `ADS_TRUE` or `ADS_FALSE` (also for NULL).
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string; `result` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalLogicalExpr(
    table: AdsHandle,
    expr: *mut Unsigned8,
    result: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalLogicalExpr");
    // SAFETY: `expr` is null or a NUL-terminated string, `result` null or
    // writable, as # Safety says.
    let (expr, result) = unsafe {
        (
            Encoding::Ansi.read_expression(expr.cast(), table),
            result.as_mut(),
        )
    };
    eval_logical(table, expr, result)
}

/// `AdsEvalLogicalExprW`: [`AdsEvalLogicalExpr`] with the expression in
/// UTF-16.
///
/// # Safety
///
/// `expr` must be null or point to NUL-terminated UTF-16; `result` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalLogicalExprW(
    table: AdsHandle,
    expr: *mut u16,
    result: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalLogicalExprW");
    // SAFETY: `expr` is null or NUL-terminated UTF-16, `result` null or
    // writable, as # Safety says.
    let (expr, result) = unsafe {
        (
            Encoding::Utf16.read_expression(expr.cast(), table),
            result.as_mut(),
        )
    };
    eval_logical(table, expr, result)
}

/// `AdsEvalNumericExpr`: the value of a numeric expression on the current
/// record; a date's is its Julian day, NULL's 0. A field's value that does
/// not fit its display format fails with [`NUMBER_DOES_NOT_FIT`], as in the
/// original engine (a Double of 1e300).
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string; `result` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalNumericExpr(
    table: AdsHandle,
    expr: *mut Unsigned8,
    result: *mut f64,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalNumericExpr");
    // SAFETY: `expr` is null or a NUL-terminated string, `result` null or
    // writable, as # Safety says.
    let (expr, result) = unsafe {
        (
            Encoding::Ansi.read_expression(expr.cast(), table),
            result.as_mut(),
        )
    };
    let (evaluation, result) = match evaluate(table, expr, result, 3) {
        Ok(evaluated) => evaluated,
        Err(code) => return code,
    };
    let number = match value(evaluation, |ty| matches!(ty, Type::Number(_) | Type::Date)) {
        Ok((Type::Number(format), Val::Number(n))) => {
            if format.fixed && !number::fits(n, format.length, format.decimals) {
                return fail(NUMBER_DOES_NOT_FIT);
            }
            n
        }
        Ok((_, Val::Date(julian))) => f64::from(julian),
        Ok(_) => 0.0,
        Err(code) => return code,
    };
    *result = number;
    AE_SUCCESS
}

/// `AdsEvalStringExpr`: the value of an expression on the current record as
/// bytes in `result` of `*len` bytes, `*len` set to their number: text as
/// it is, other types but numbers and logical values (dates, times, raw
/// data, `;` joins) as their index keys, as in the original engine. A buffer
/// that is too small gets what fits and `AE_INSUFFICIENT_BUFFER` with the
/// length needed.
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string; `result` must
/// be null or hold `*len` bytes; `len` must be null or valid for reads and
/// writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalStringExpr(
    table: AdsHandle,
    expr: *mut Unsigned8,
    result: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalStringExpr");
    // SAFETY: `expr` is null or a NUL-terminated string, as # Safety says.
    let expr = unsafe { Encoding::Ansi.read_expression(expr.cast(), table) };
    let (evaluation, ()) = match evaluate(table, expr, (!len.is_null()).then_some(()), 4) {
        Ok(evaluated) => evaluated,
        Err(code) => return code,
    };
    let bytes_of = |ty: Type| !matches!(ty, Type::Number(_) | Type::Logical | Type::Null);
    match value(evaluation, bytes_of) {
        Ok((ty, value)) => {
            let bytes = value.bytes(ty).unwrap_or_default();
            // SAFETY: `result` and `len` are as # Safety says.
            unsafe { write_cut_at(&bytes, result, len, 3) }
        }
        Err(code) => code,
    }
}

/// `AdsEvalTestExpr`: the type of an expression's result: `ADS_LOGICAL`,
/// `ADS_NUMERIC` (also dates), `ADS_STRING` or `ADS_RAW` (times, raw data,
/// `;` joins). It needs no current record. The `NULL` literal, which has no
/// type, fails with `AE_INVALID_EXPRESSION`.
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string; `ty` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalTestExpr(
    table: AdsHandle,
    expr: *mut Unsigned8,
    ty: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalTestExpr");
    // SAFETY: `expr` is null or a NUL-terminated string, `ty` null or
    // writable, as # Safety says.
    let (expr, ty) = unsafe {
        (
            Encoding::Ansi.read_expression(expr.cast(), table),
            ty.as_mut(),
        )
    };
    let (evaluation, ty) = match evaluate(table, expr, ty, 3) {
        Ok(evaluated) => evaluated,
        Err(code) => return code,
    };
    match evaluation.ty.ace_type() {
        Some(code) => {
            *ty = code;
            AE_SUCCESS
        }
        None => fail(AE_INVALID_EXPRESSION),
    }
}

/// `AdsIsExprValid`: whether an expression compiles for a table, in
/// `*valid`. As in the original engine, an expression that does not still
/// succeeds, with `ADS_FALSE` and the reason as the last error (3401 to
/// 3425).
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string; `valid` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsExprValid(
    table: AdsHandle,
    expr: *mut Unsigned8,
    valid: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsExprValid");
    // SAFETY: `expr` is null or a NUL-terminated string, `valid` null or
    // writable, as # Safety says.
    let (expr, valid) = unsafe {
        (
            Encoding::Ansi.read_expression(expr.cast(), table),
            valid.as_mut(),
        )
    };
    let backend = match table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let Some(text) = expr else {
        return null_param(2);
    };
    let Some(valid) = valid else {
        return null_param(3);
    };
    let ok = match backend.check_expression(table, text.engine) {
        Ok(()) => true,
        Err(tflip_engine::Error::Expression(kind)) => {
            fail(kind.code(VALIDITY_ERRORS));
            false
        }
        Err(tflip_engine::Error::InvalidExpression) => {
            fail(AE_INVALID_EXPRESSION);
            false
        }
        Err(error) => return error_code(error),
    };
    *valid = if ok { ADS_TRUE } else { ADS_FALSE };
    AE_SUCCESS
}

/// `AdsLocate`: moves to the first record, from the top (`forward`) or from
/// the bottom, that passes the table's filters and for which `expr` holds;
/// `*found` says whether there is one. If not, the table goes to EOF (BOF
/// backwards).
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string; `found` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsLocate(
    table: AdsHandle,
    expr: *mut Unsigned8,
    forward: Unsigned16,
    found: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsLocate");
    // SAFETY: `expr` is null or a NUL-terminated string, `found` null or
    // writable, as # Safety says.
    let (expr, found) = unsafe {
        (
            Encoding::Ansi.read_expression(expr.cast(), table),
            found.as_mut(),
        )
    };
    let backend = match table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let (Some(text), Some(found)) = (expr, found) else {
        return fail(AE_INVALID_POINTER);
    };
    match backend.locate(table, text.engine, forward != 0) {
        Ok(located) => {
            *found = if located { ADS_TRUE } else { ADS_FALSE };
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsContinue`: goes on with the table's last `AdsLocate`, from the
/// record after the current one in its direction, and sets `*found`; past
/// the last record the table is at EOF (or BOF). Without an `AdsLocate`
/// before it fails with `AE_LOCATE_NOT_CALLED`, as in the original engine.
///
/// # Safety
///
/// `found` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsContinue(table: AdsHandle, found: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsContinue");
    let backend = match table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    // SAFETY: `found` is null or writable, as # Safety says.
    let Some(found) = (unsafe { found.as_mut() }) else {
        return fail(AE_INVALID_POINTER);
    };
    match backend.continue_locate(table) {
        Ok(located) => {
            *found = if located { ADS_TRUE } else { ADS_FALSE };
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsExpressionLongToShort90`: the expression `long_expr` with the table's
/// field names replaced by `F<n>`, the form index files store (`id > 30`
/// becomes `F1 > 30`), in `short_expr` of `*len` bytes (NUL-terminated);
/// `*len` is set to its length. ARC calls it before setting a filter. See
/// [`tflip_engine::table::Table::with_field_numbers`] for what is replaced.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsExpressionLongToShort90( ADSHANDLE hTbl, UNSIGNED8
/// *pucLongExpr, UNSIGNED8 *pucShortExpr, UNSIGNED32 *pulLen )`.
///
/// As in the original engine: a buffer that is too small fails with
/// `AE_INSUFFICIENT_BUFFER` and `*len` set to the length needed including the
/// NUL; a handle that is not a table (an index order among them) fails with
/// `AE_INVALID_TABLE_HANDLE`, while `AdsGetLastError` reports
/// `AE_INVALID_HANDLE` for one that is not a handle at all.
///
/// # Safety
///
/// `long_expr` must be null or point to a NUL-terminated string;
/// `short_expr` must be null or hold `*len` bytes; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExpressionLongToShort90(
    table: AdsHandle,
    long_expr: *mut Unsigned8,
    short_expr: *mut Unsigned8,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExpressionLongToShort90");
    let backend = api(table);
    match backend.handle_kind(table) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => {}
        Ok(Some(_)) => return fail_handle(AE_INVALID_TABLE_HANDLE),
        Ok(None) => {
            fail_handle(AE_INVALID_HANDLE);
            return AE_INVALID_TABLE_HANDLE;
        }
        Err(error) => return error_code(error),
    }
    if short_expr.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `long_expr` is null or a NUL-terminated string, as # Safety
    // says.
    let Some(long) = (unsafe { c_str(long_expr) }) else {
        return fail(AE_INVALID_POINTER);
    };
    let short = match backend.expression_to_short(table, ansi::data_in(long)) {
        Ok(short) => ansi::data_out(&short),
        Err(error) => return error_code(error),
    };
    // SAFETY: neither is null, and `short_expr` holds `*len` bytes, as # Safety
    // says.
    unsafe { write_name(&short, short_expr, len) }
}

/// `AdsExpressionShortToLong90`: the expression `short_expr` with each `F<n>`
/// replaced by the name of the table's field *n* (`F1 > 30` becomes
/// `id > 30`), in `long_expr` of `*len` bytes (NUL-terminated); `*len` is set
/// to its length. ARC calls it on filters. See
/// [`tflip_engine::table::Table::with_field_names`] for what is replaced.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsExpressionShortToLong90( ADSHANDLE hTbl, UNSIGNED8
/// *pucShortExpr, UNSIGNED8 *pucLongExpr, UNSIGNED32 *pulLen )`.
///
/// As in the original engine: a field number the table does not have fails
/// with `AE_INVALID_FIELDNUMBER`; a buffer that is too small fails with
/// `AE_INSUFFICIENT_BUFFER` and `*len` set to the length needed including the
/// NUL, after the bytes that fit are copied (without a NUL); any handle that
/// is not a table fails with `AE_INVALID_TABLE_HANDLE`.
///
/// # Safety
///
/// `short_expr` must be null or point to a NUL-terminated string;
/// `long_expr` must be null or hold `*len` bytes; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExpressionShortToLong90(
    table: AdsHandle,
    short_expr: *mut Unsigned8,
    long_expr: *mut Unsigned8,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExpressionShortToLong90");
    let backend = api(table);
    match backend.handle_kind(table) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => {}
        Err(error) => return error_code(error),
        Ok(_) => return fail_handle(AE_INVALID_TABLE_HANDLE),
    }
    if long_expr.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `short_expr` is null or a NUL-terminated string, as # Safety
    // says.
    let Some(short) = (unsafe { c_str(short_expr) }) else {
        return fail(AE_INVALID_POINTER);
    };
    let long = match backend.expression_to_long(table, ansi::data_in(short)) {
        Ok(long) => ansi::data_out(&long),
        Err(error) => return error_code(error),
    };
    // SAFETY: neither is null, so `long_expr` holds `*len` bytes and `len`
    // is readable and writable, as # Safety says.
    unsafe { write_units(&long, long_expr, len) }
}

/// Calls a `90` expression function with a 32-bit length for one of the
/// older functions with a 16-bit one. As in the original engine, a buffer of
/// size 0 fails with `AE_INSUFFICIENT_BUFFER` and leaves `*len` at 0.
///
/// # Safety
///
/// `len` must be null or valid for reads and writes; the rest as for `f`.
unsafe fn with_short_length(
    len: *mut Unsigned16,
    f: impl FnOnce(*mut Unsigned32) -> Unsigned32,
) -> Unsigned32 {
    if len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: not null, so readable, as the caller promises.
    let mut long_len = Unsigned32::from(unsafe { *len });
    let rc = f(&mut long_len);
    // SAFETY: not null, so readable and writable, as the caller promises.
    unsafe {
        if *len != 0 {
            *len = Unsigned16::try_from(long_len).unwrap_or(Unsigned16::MAX);
        }
    }
    rc
}

/// `AdsExpressionLongToShort`: [`AdsExpressionLongToShort90`] with a 16-bit
/// length.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsExpressionLongToShort( ADSHANDLE hTbl, UNSIGNED8
/// *pucLongExpr, UNSIGNED8 *pucShortExpr, UNSIGNED16 *pusLen )`.
///
/// # Safety
///
/// As for [`AdsExpressionLongToShort90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExpressionLongToShort(
    table: AdsHandle,
    long_expr: *mut Unsigned8,
    short_expr: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExpressionLongToShort");
    // SAFETY: the pointers are as # Safety says; `with_short_length` hands
    // the call a length of its own.
    unsafe {
        with_short_length(len, |len| {
            AdsExpressionLongToShort90(table, long_expr, short_expr, len)
        })
    }
}

/// `AdsExpressionShortToLong`: [`AdsExpressionShortToLong90`] with a 16-bit
/// length.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsExpressionShortToLong( ADSHANDLE hTbl, UNSIGNED8
/// *pucShortExpr, UNSIGNED8 *pucLongExpr, UNSIGNED16 *pusLen )`.
///
/// # Safety
///
/// As for [`AdsExpressionShortToLong90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExpressionShortToLong(
    table: AdsHandle,
    short_expr: *mut Unsigned8,
    long_expr: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExpressionShortToLong");
    // SAFETY: the pointers are as # Safety says; `with_short_length` hands
    // the call a length of its own.
    unsafe {
        with_short_length(len, |len| {
            AdsExpressionShortToLong90(table, short_expr, long_expr, len)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use crate::{
        ADS_IGNOREFILTERS, AE_INSUFFICIENT_BUFFER, AE_NO_CURRENT_RECORD, AdsGetLastError, fixtures,
    };
    use crate::{AdsGetRecordNum, AdsGotoEOF, AdsGotoRecord};

    fn last_error() -> Unsigned32 {
        let (mut code, mut buf, mut len) = (0, [0u8; 600], 600);
        unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        code
    }

    /// What the original engine answers on `adi_keys/keys.adt` (see
    /// `fixtures/expr/expressions.json`).
    #[test]
    fn expressions_on_the_current_record() {
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        assert_eq!(AdsGotoRecord(table, 4), AE_SUCCESS);

        let ty = |e: &str| {
            let mut ty = 0;
            let rc = unsafe { AdsEvalTestExpr(table, text(e).as_ptr().cast_mut().cast(), &mut ty) };
            (rc, ty)
        };
        assert_eq!(ty("k_date + 1"), (AE_SUCCESS, 2));
        assert_eq!(ty("k_char;k_integer"), (AE_SUCCESS, 16));
        assert_eq!(ty("nosuchfield"), (3511, 0));
        assert_eq!(ty("NULL"), (AE_INVALID_EXPRESSION, 0));

        let logical = |e: &str| {
            let mut b = 9;
            let rc =
                unsafe { AdsEvalLogicalExpr(table, text(e).as_ptr().cast_mut().cast(), &mut b) };
            (rc, b)
        };
        assert_eq!(
            logical("k_integer > 3 .AND. k_logical"),
            (AE_SUCCESS, ADS_TRUE)
        );
        assert_eq!(
            logical("k_integer"),
            (AE_INVALID_EXPRESSION, 9),
            "not logical"
        );

        let numeric = |e: &str| {
            let mut n = -1.0;
            let rc =
                unsafe { AdsEvalNumericExpr(table, text(e).as_ptr().cast_mut().cast(), &mut n) };
            (rc, n)
        };
        assert_eq!(numeric("k_integer + 1"), (AE_SUCCESS, 2147483648.0));
        assert_eq!(
            numeric("k_double").0,
            NUMBER_DOES_NOT_FIT,
            "1e300 in a Double field"
        );
        assert_eq!(numeric("1/0").0, 4420);

        let string = |e: &str| {
            let (mut buf, mut len) = ([0u8; 64], 64);
            let e = text(e);
            let rc = unsafe {
                AdsEvalStringExpr(
                    table,
                    e.as_ptr().cast_mut().cast(),
                    buf.as_mut_ptr(),
                    &mut len,
                )
            };
            (rc, buf[..usize::from(len)].to_vec())
        };
        assert_eq!(
            string("DTOS(k_date) + STR(k_short, 6)"),
            (AE_SUCCESS, b"99991231 32767".to_vec())
        );
        assert_eq!(
            string("k_date"),
            (AE_SUCCESS, vec![0xc1, 0x54, 0x7f, 0x8b, 0, 0, 0, 0])
        );

        let valid = |e: &str| {
            let mut valid = 9;
            let rc =
                unsafe { AdsIsExprValid(table, text(e).as_ptr().cast_mut().cast(), &mut valid) };
            (rc, valid, last_error())
        };
        assert_eq!(valid("UPPER(k_char)"), (AE_SUCCESS, ADS_TRUE, 0));
        assert_eq!(valid("UPPER("), (AE_SUCCESS, ADS_FALSE, 3418));

        // The type is checked before the position.
        assert_eq!(AdsGotoEOF(table), AE_SUCCESS);
        assert_eq!(logical("k_logical"), (AE_NO_CURRENT_RECORD, 9));
        assert_eq!(logical("k_integer"), (AE_INVALID_EXPRESSION, 9));

        let locate = |e: &str, forward| {
            let mut found = 9;
            let e = text(e);
            let rc = unsafe { AdsLocate(table, e.as_ptr().cast_mut().cast(), forward, &mut found) };
            let mut record = 0;
            unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut record) };
            (rc, found, record)
        };
        assert_eq!(locate("k_integer < 0", 1), (AE_SUCCESS, ADS_TRUE, 2));
        assert_eq!(locate("k_integer < 0", 0), (AE_SUCCESS, ADS_TRUE, 5));
        assert_eq!(
            locate("k_integer > 2147483647", 1),
            (AE_SUCCESS, ADS_FALSE, 7)
        );
        assert_eq!(locate("nosuchfield", 1).0, 3511);
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    #[test]
    fn expressions_long_to_short() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let to_short = |handle, long: &CStr, size| {
            let mut buf = [b'#'; 16];
            let mut len = size;
            let (long, short) = (long.as_ptr().cast_mut().cast(), buf.as_mut_ptr());
            let rc = unsafe { AdsExpressionLongToShort90(handle, long, short, &mut len) };
            (rc, len, buf)
        };
        let (rc, len, buf) = to_short(table, c"name > 'a'", 9);
        assert_eq!((rc, len, &buf[..9]), (AE_SUCCESS, 8, &b"F2 > 'a'\0"[..]));
        // As in the original engine: the size needed includes the NUL, and
        // the buffer is left alone.
        let (rc, len, buf) = to_short(table, c"name > 'a'", 8);
        assert_eq!((rc, len, buf), (AE_INSUFFICIENT_BUFFER, 9, [b'#'; 16]));

        let mut order = 0;
        let name = c"PK_INDEXED".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsGetIndexHandle(table, name, &mut order) },
            AE_SUCCESS
        );
        for handle in [connect, order, 0x12345] {
            // A successful call first: a failed handle check keeps a pending
            // error.
            assert_eq!(crate::AdsCacheOpenCursors(25), crate::AE_SUCCESS);
            let (rc, len, _) = to_short(handle, c"id", 16);
            assert_eq!((rc, len), (AE_INVALID_TABLE_HANDLE, 16));
            let mut code = 0;
            let (buf, len) = (std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(
                unsafe { crate::AdsGetLastError(&mut code, buf, len) },
                AE_SUCCESS
            );
            let last = if handle == 0x12345 {
                AE_INVALID_HANDLE
            } else {
                AE_INVALID_TABLE_HANDLE
            };
            assert_eq!(code, last, "the last error for {handle:#x}");
        }
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn expressions_short_to_long() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let to_long = |handle, short: &CStr, size| {
            let mut buf = [b'#'; 16];
            let mut len = size;
            let (short, long) = (short.as_ptr().cast_mut().cast(), buf.as_mut_ptr());
            let rc = unsafe { AdsExpressionShortToLong90(handle, short, long, &mut len) };
            (rc, len, buf)
        };
        let (rc, len, buf) = to_long(table, c"F1>30;F5", 11);
        assert_eq!(
            (rc, len, &buf[..11]),
            (AE_SUCCESS, 10, &b"id>30;born\0"[..])
        );
        // As in the original engine: what fits is copied, without a NUL.
        let (rc, len, buf) = to_long(table, c"F2", 4);
        assert_eq!(
            (rc, len, &buf[..5]),
            (AE_INSUFFICIENT_BUFFER, 5, &b"name#"[..])
        );
        let (rc, len, _) = to_long(table, c"F7", 16);
        assert_eq!((rc, len), (crate::AE_INVALID_FIELDNUMBER, 16));
        for handle in [connect, 0x12345] {
            assert_eq!(to_long(handle, c"F1", 16).0, AE_INVALID_TABLE_HANDLE);
        }
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// The 16-bit variants: as the `90` ones, except that a buffer of size 0
    /// leaves the length at 0.
    #[test]
    fn expressions_with_16_bit_lengths() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        type Convert =
            unsafe extern "system" fn(AdsHandle, *mut u8, *mut u8, *mut Unsigned16) -> Unsigned32;
        let call = |f: Convert, expr: &CStr, size| {
            let mut buf = [b'#'; 16];
            let mut len: u16 = size;
            let rc = unsafe {
                f(
                    table,
                    expr.as_ptr().cast_mut().cast(),
                    buf.as_mut_ptr(),
                    &mut len,
                )
            };
            (rc, len, buf)
        };
        let (rc, len, buf) = call(AdsExpressionLongToShort, c"name > 'a'", 16);
        assert_eq!((rc, len, &buf[..9]), (AE_SUCCESS, 8, &b"F2 > 'a'\0"[..]));
        let (rc, len, buf) = call(AdsExpressionShortToLong, c"F2 > 'a'", 5);
        assert_eq!(
            (rc, len, &buf[..6]),
            (AE_INSUFFICIENT_BUFFER, 11, &b"name #"[..])
        );
        let (rc, len, _) = call(AdsExpressionLongToShort, c"name > 'a'", 0);
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 0));
        let (rc, len, _) = call(AdsExpressionShortToLong, c"F9", 16);
        assert_eq!((rc, len), (crate::AE_INVALID_FIELDNUMBER, 16));
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// What the probe records of an index created on `table` in `file`
    /// with the key `expression` and the condition `condition`: its
    /// expression and condition, the key of record 197, its key count, and
    /// the expression and condition once it is opened again.
    fn created_index(
        table: AdsHandle,
        file: &str,
        expression: &[u8],
        condition: &[u8],
    ) -> serde_json::Map<String, serde_json::Value> {
        use crate::{ADS_RESPECTFILTERS, AdsGetIndexCondition, AdsGetIndexExpr};
        use crate::{AdsCloseIndex, AdsExtractKey, AdsGetKeyCount, AdsOpenIndex};
        use serde_json::json;

        let nul = |bytes: &[u8]| -> Vec<u8> { [bytes, &[0]].concat() };
        let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
        let text = |f: unsafe extern "system" fn(AdsHandle, *mut u8, *mut u16) -> u32,
                    index: AdsHandle| {
            let (mut buf, mut len) = ([0u8; 256], 256);
            assert_eq!(unsafe { f(index, buf.as_mut_ptr(), &mut len) }, AE_SUCCESS);
            hex(&buf[..usize::from(len)])
        };
        let file = std::ffi::CString::new(file).unwrap();
        let (mut expression, mut condition) = (nul(expression), nul(condition));
        let mut order = 0;
        let rc = unsafe {
            crate::AdsCreateIndex(
                table,
                file.as_ptr().cast_mut().cast(),
                file.as_ptr().cast_mut().cast(),
                expression.as_mut_ptr(),
                if condition.len() > 1 {
                    condition.as_mut_ptr()
                } else {
                    std::ptr::null_mut()
                },
                std::ptr::null_mut(),
                0,
                &mut order,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let mut got = serde_json::Map::new();
        got.insert("rc".into(), json!(0));
        got.insert("expr".into(), json!(text(AdsGetIndexExpr, order)));
        got.insert("condition".into(), json!(text(AdsGetIndexCondition, order)));
        AdsGotoRecord(table, 197);
        let (mut key, mut len) = ([0u8; 256], 256);
        unsafe { AdsExtractKey(order, key.as_mut_ptr(), &mut len) };
        got.insert("key".into(), json!(hex(&key[..usize::from(len)])));
        let mut count = 0;
        unsafe { AdsGetKeyCount(order, ADS_RESPECTFILTERS, &mut count) };
        got.insert("keys".into(), json!(count));
        AdsCloseIndex(order);
        let (mut orders, mut n) = ([0; 4], 4);
        let name = file.as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsOpenIndex(table, name, orders.as_mut_ptr(), &mut n) },
            AE_SUCCESS
        );
        got.insert("reopened".into(), json!(text(AdsGetIndexExpr, orders[0])));
        let reopened = text(AdsGetIndexCondition, orders[0]);
        got.insert("reopened_condition".into(), json!(reopened));
        AdsCloseIndex(orders[0]);
        got
    }

    /// Replays `fixtures/expr/oem.json` (see `tools/ace/oem`): the text a
    /// client gives in filters and expressions about an OEM table, as the
    /// original engine converts it to the table's code page.
    #[test]
    fn oem_tables_convert_client_text() {
        use crate::filter::{AdsClearAOF, AdsClearFilter, AdsGetAOF, AdsGetFilter};
        use crate::filter::{AdsIsRecordInAOF, AdsSetAOF, AdsSetFilter, AdsSetFilter100};
        use crate::{ADS_NTX, ADS_OEM, ADS_RESPECTFILTERS, AdsGetRecordCount, AdsGotoTop};
        use serde_json::{Map, Value as Json, json};

        let _settings = fixtures::settings_shared();
        let path = fixtures::dir("../expr/oem.json");
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        // Indexes are created in a copy, with the table open exclusively.
        let source = fixtures::scratch("oem_tables_convert_client_text", "ntx/charset_default");
        let connect = fixtures::connect_to(&source);
        let mut table = 0;
        let null = std::ptr::null_mut();
        let rc = unsafe {
            crate::AdsOpenTable(
                connect,
                c"co.dbf".as_ptr().cast_mut().cast(),
                null,
                ADS_NTX,
                ADS_OEM,
                0,
                1,
                1,
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let unhex = |hex: &str| -> Vec<u8> {
            (0..hex.len() / 2 * usize::from(hex != "-"))
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
                .collect()
        };
        let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
        let mut failures = Vec::new();
        let mut previous = "";
        for step in &steps {
            let command = step["command"].as_str().unwrap();
            let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
            if verb == "index" {
                let words: Vec<&str> = arg.split(' ').collect();
                let got = created_index(table, words[0], &unhex(words[1]), &unhex(words[2]));
                let mut expected = step.as_object().unwrap().clone();
                expected.remove("command");
                if got != expected {
                    failures.push(format!("{command}: {got:?}, expected {expected:?}"));
                }
                continue;
            }
            let mut text = unhex(arg);
            // The probe's text ends in two NULs, a character of UTF-16.
            text.extend([0, 0]);
            let (mut flag, mut bytes) = (None, None);
            let (mut counted, mut located) = (false, false);
            let rc = unsafe {
                let ptr = text.as_mut_ptr();
                match verb {
                    "goto" => AdsGotoRecord(table, arg.parse().unwrap()),
                    "eval" => {
                        let mut holds = 0;
                        let rc = AdsEvalLogicalExpr(table, ptr, &mut holds);
                        flag = Some(holds);
                        rc
                    }
                    "weval" => {
                        let mut holds = 0;
                        let rc = AdsEvalLogicalExprW(table, ptr.cast(), &mut holds);
                        flag = Some(holds);
                        rc
                    }
                    "valid" => {
                        let mut valid = 0;
                        let rc = AdsIsExprValid(table, ptr, &mut valid);
                        flag = Some(valid);
                        rc
                    }
                    "str" => {
                        let (mut buf, mut len) = ([0u8; 256], 256);
                        let rc = AdsEvalStringExpr(table, ptr, buf.as_mut_ptr(), &mut len);
                        bytes = Some(buf[..usize::from(len)].to_vec());
                        rc
                    }
                    "filter" | "wfilter" | "ufilter" | "aof" => {
                        counted = true;
                        match verb {
                            "filter" => AdsSetFilter(table, ptr),
                            "wfilter" => AdsSetFilter100(table, ptr.cast(), 0x2000),
                            "ufilter" => AdsSetFilter100(table, ptr.cast(), 0x4000),
                            _ => AdsSetAOF(table, ptr, 1),
                        }
                    }
                    "clear" => {
                        counted = true;
                        AdsClearFilter(table);
                        AdsClearAOF(table)
                    }
                    "getfilter" | "getaof" => {
                        let (mut buf, mut len) = ([0u8; 256], 256);
                        let rc = match verb {
                            "getaof" => AdsGetAOF(table, buf.as_mut_ptr(), &mut len),
                            _ => AdsGetFilter(table, buf.as_mut_ptr(), &mut len),
                        };
                        bytes = Some(buf[..usize::from(len)].to_vec());
                        rc
                    }
                    "inaof" => {
                        let mut is_in = 0;
                        let rc = AdsIsRecordInAOF(table, arg.parse().unwrap(), &mut is_in);
                        flag = Some(is_in);
                        rc
                    }
                    "locate" => {
                        let mut found = 0;
                        located = true;
                        let rc = AdsLocate(table, ptr, 1, &mut found);
                        flag = Some(found);
                        rc
                    }
                    other => panic!("unknown command {other}"),
                }
            };
            let mut got = Map::new();
            got.insert("rc".into(), json!(rc));
            if rc != AE_SUCCESS {
                got.insert("error".into(), json!(last_error()));
            } else {
                if let Some(flag) = flag {
                    got.insert("flag".into(), json!(flag != 0));
                }
                if let Some(bytes) = &bytes {
                    got.insert("bytes".into(), json!(hex(bytes)));
                }
            }
            let mut number = 0;
            if counted {
                AdsGotoTop(table);
                let mut count = 0;
                unsafe { AdsGetRecordCount(table, ADS_RESPECTFILTERS, &mut count) };
                got.insert("count".into(), json!(count));
                unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut number) };
                got.insert("first".into(), json!(number));
                let (mut buf, mut len) = ([0u8; 16], 16);
                let field = c"C".as_ptr().cast_mut().cast();
                let rc = unsafe { AdsEvalStringExpr(table, field, buf.as_mut_ptr(), &mut len) };
                if rc == AE_SUCCESS {
                    got.insert("c".into(), json!(hex(&buf[..usize::from(len)])));
                }
            }
            if located {
                unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut number) };
                got.insert("recno".into(), json!(number));
            }
            let mut expected = step.as_object().unwrap().clone();
            expected.remove("command");
            // The original hands a filter set in UTF-8 or UTF-16 back in
            // UTF-8; tflip's text is in the ANSI code page (see
            // docs/KNOWN_ISSUES.md).
            let unicode_set = previous == "wfilter" || previous == "ufilter";
            let skipped = verb == "getfilter" && unicode_set;
            if !skipped && got != expected {
                failures.push(format!("{command}: {got:?}, expected {expected:?}"));
            }
            previous = verb;
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        crate::AdsDisconnect(connect);
    }

    /// What the original engine does with a division by 0 in the branch an
    /// `IIF` does not take (`tools/ace/expr/iif.c`): it evaluates both, and
    /// the failure is 4420 as an expression, 4020 in an index's key (which
    /// is then not created) or an AOF, 4120 in a filter.
    #[test]
    fn division_by_zero_in_the_branch_not_taken() {
        use crate::filter::{AdsSetAOF, AdsSetFilter};
        use crate::{ADS_ADT, ADS_ANSI, ADS_PROPRIETARY_LOCKING, AdsCreateIndex, AdsSkip};

        let source = fixtures::scratch("division_by_zero_in_the_branch_not_taken", "indexes");
        let connect = fixtures::connect_to(&source);
        let mut table = 0;
        let name = c"indexed.adt".as_ptr().cast_mut().cast();
        let null = std::ptr::null_mut();
        let rc = unsafe {
            crate::AdsOpenTable(
                connect,
                name,
                null,
                ADS_ADT,
                ADS_ANSI,
                ADS_PROPRIETARY_LOCKING,
                1,
                0,
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let expr = c"IIF(RECNO()=2,0,100/(RECNO()-2))";
        let text = expr.as_ptr().cast_mut().cast();
        let mut number = 0.0;
        AdsGotoRecord(table, 3);
        assert_eq!(
            unsafe { AdsEvalNumericExpr(table, text, &mut number) },
            AE_SUCCESS
        );
        assert_eq!(number, 100.0);
        AdsGotoRecord(table, 2);
        assert_eq!(
            unsafe { AdsEvalNumericExpr(table, text, &mut number) },
            4420
        );
        let mut index = 0;
        let rc = unsafe {
            AdsCreateIndex(
                table,
                c"divides.adi".as_ptr().cast_mut().cast(),
                c"divides".as_ptr().cast_mut().cast(),
                text,
                null,
                null,
                0,
                &mut index,
            )
        };
        assert_eq!(rc, 4020);
        assert!(!std::path::Path::new(&source).join("divides.adi").exists());
        let filter = c"IIF(RECNO()=2,0,100/(RECNO()-2))<>0";
        assert_eq!(
            unsafe { AdsSetFilter(table, filter.as_ptr().cast_mut().cast()) },
            AE_SUCCESS
        );
        crate::AdsGotoTop(table);
        assert_eq!(AdsSkip(table, 1), 4120);
        assert_eq!(
            unsafe { AdsSetAOF(table, filter.as_ptr().cast_mut().cast(), 1) },
            4020
        );
        // An index that exists fails the write of the record it divides by 0 on.
        crate::AdsClearAOF(table);
        crate::AdsClearFilter(table);
        let key = c"IIF(id=7777,0,100/(id-7777))";
        let rc = unsafe {
            AdsCreateIndex(
                table,
                c"divides.adi".as_ptr().cast_mut().cast(),
                c"divides".as_ptr().cast_mut().cast(),
                key.as_ptr().cast_mut().cast(),
                null,
                null,
                0,
                &mut index,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(crate::AdsAppendRecord(table), AE_SUCCESS);
        assert_eq!(
            unsafe { crate::AdsSetLong(table, c"id".as_ptr().cast_mut().cast(), 7777) },
            AE_SUCCESS
        );
        assert_eq!(crate::AdsWriteRecord(table), 4020);
        crate::AdsDisconnect(connect);
    }

    crate::fixtures::remote_twins!(
        expressions_long_to_short,
        expressions_short_to_long,
        expressions_on_the_current_record,
        oem_tables_convert_client_text,
        division_by_zero_in_the_branch_not_taken
    );
}
