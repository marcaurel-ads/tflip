//! Relations (see [`tflip_engine::relation`]): a child table following a
//! parent's moves through one of its orders.

use tflip_engine::handle::HandleKind;

use crate::backend::api;
use crate::ffi::text;
use crate::filter::table_of;
use crate::last_error::{Call, error_text, fail_text};
use crate::{
    AE_INVALID_HANDLE, AE_INVALID_INDEX_HANDLE, AE_INVALID_POINTER, AE_SUCCESS, AdsHandle,
    Unsigned8, Unsigned32, code, error_code,
};

/// Sets the relation from `parent` to the order `child`, as the original
/// engine checks it: the parent a table, the child an index order (else
/// `AE_INVALID_INDEX_HANDLE`, with the text of `AE_INVALID_HANDLE` for a
/// handle that is nothing), an expression given (`None` for null).
fn set(parent: AdsHandle, child: AdsHandle, expr: Option<String>, scoped: bool) -> Unsigned32 {
    let backend = match table_of(parent) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    match api(child).handle_kind(child) {
        Ok(Some(HandleKind::Index)) => {}
        Ok(Some(_)) => {
            let text = error_text(AE_INVALID_INDEX_HANDLE).unwrap_or_default();
            return fail_text(
                AE_INVALID_INDEX_HANDLE,
                format!(
                    "Error {AE_INVALID_INDEX_HANDLE}:  {text}  The handle must be to an index \
                     order."
                ),
            );
        }
        _ => {
            let text = error_text(AE_INVALID_HANDLE).unwrap_or_default();
            return fail_text(
                AE_INVALID_INDEX_HANDLE,
                format!("Error {AE_INVALID_HANDLE}:  {text}  The given handle is not readable."),
            );
        }
    }
    let Some(expression) = expr else {
        let text = error_text(AE_INVALID_POINTER).unwrap_or_default();
        return fail_text(
            AE_INVALID_POINTER,
            format!("Error {AE_INVALID_POINTER}:  {text}  Parameter 3"),
        );
    };
    let empty = expression.is_empty();
    match backend.set_relation(parent, child, expression, scoped) {
        Ok(()) => AE_SUCCESS,
        Err(tflip_engine::Error::InvalidExpression) if empty => crate::last_error::fail_detail(
            crate::AE_INVALID_EXPRESSION,
            "Empty expressions are not valid.",
        ),
        Err(error) => error_code(error),
    }
}

/// `AdsSetRelation`: makes the order `index_child`'s table follow
/// `table_parent`'s moves, seeking the key `expr` gives on the parent's
/// record.
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetRelation(
    table_parent: AdsHandle,
    index_child: AdsHandle,
    expr: *mut Unsigned8,
) -> Unsigned32 {
    let _call = Call::begin("AdsSetRelation");
    // SAFETY: `expr` is null or NUL-terminated, as # Safety says.
    let expr = unsafe { text(expr) };
    set(table_parent, index_child, expr, false)
}

/// `AdsSetScopedRelation`: as `AdsSetRelation`, and scopes the child's
/// order to the key.
///
/// # Safety
///
/// `expr` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetScopedRelation(
    table_parent: AdsHandle,
    index_child: AdsHandle,
    expr: *mut Unsigned8,
) -> Unsigned32 {
    let _call = Call::begin("AdsSetScopedRelation");
    // SAFETY: `expr` is null or NUL-terminated, as # Safety says.
    let expr = unsafe { text(expr) };
    set(table_parent, index_child, expr, true)
}

/// `AdsClearRelation`: clears the relations of `table_parent`; the
/// children stay where they are, and a scoped relation's scope stays.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearRelation(table_parent: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsClearRelation");
    let backend = match table_of(table_parent) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    code(backend.clear_relation(table_parent))
}
