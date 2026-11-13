//! Relations (`AdsSetRelation`, `AdsSetScopedRelation`,
//! `AdsClearRelation`): a child table follows a parent table through one of
//! its orders, as the original engine does it (`fixtures/write/relations`).
//!
//! When the parent's position changes, its children are told; a child
//! follows at its next use (a move, a read, its position asked; not
//! `AdsGetScope`): it seeks the key the relation's expression gives on the
//! parent's record in its order, and with a scoped relation sets the
//! order's scope to that key first. A parent off its records sends the child
//! to EOF, its found flag left as it was. Between the parent's moves the
//! child moves freely; a move of the parent that leaves it where it was
//! (`AdsGotoRecord` to its record, `AdsSkip(0)`, a refresh) is no move.
//!
//! A parent has one relation per child table (setting another replaces
//! it), and a table cannot descend from itself (`AE_CYCLIC_RELATION`).
//! Clearing a parent's relations leaves its children where they are, and a
//! scoped relation's scope stays.

use std::sync::{Arc, Weak};

use crate::handle::Handle;
use crate::index::{self, Index};
use crate::seek::{SeekType, SeekValue};
use crate::table::{self, Table};
use crate::{Error, Session, lock};
use tflip_storage::order::Kind;

/// A relation from a parent table to an order of a child table.
#[derive(Debug)]
pub(crate) struct Relation {
    order: Weak<Index>,
    /// The child: the order's table.
    child: Weak<Table>,
    parent: Weak<Table>,
    /// The expression giving the key, on the parent's records.
    expression: String,
    /// Whether the child's order is scoped to the key.
    scoped: bool,
}

/// Sets a relation from the table `parent` to the order `order` of its
/// child (`AdsSetRelation`, or `AdsSetScopedRelation` if `scoped`), keyed by
/// `expression` on the parent's records, which must be valid there. The
/// child follows the parent at its next use.
pub fn set(
    session: &Session,
    parent: Handle,
    order: Handle,
    expression: &str,
    scoped: bool,
) -> Result<(), Error> {
    let parent = table::get(session, parent)?;
    let order = index::get(session, order)?;
    let child = Arc::clone(order.owner());
    // One relation is set at a time in the process, so that two cannot
    // make a cycle between them.
    static SETTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _setting = lock(&SETTING);
    if Arc::ptr_eq(&child, &parent) {
        return Err(Error::CyclicRelation(true));
    }
    if descends(&child, &parent) {
        return Err(Error::CyclicRelation(false));
    }
    parent.compile(expression)?;
    let relation = Arc::new(Relation {
        order: Arc::downgrade(&order),
        child: Arc::downgrade(&child),
        parent: Arc::downgrade(&parent),
        expression: expression.to_owned(),
        scoped,
    });
    let mut relations = lock(&parent.relations);
    relations.retain(|r| !r.child.upgrade().is_some_and(|c| Arc::ptr_eq(&c, &child)));
    relations.push(Arc::clone(&relation));
    drop(relations);
    *lock(&child.following) = Some(relation);
    Ok(())
}

/// Whether `table` is `of` or one of its relations' descendants.
fn descends(of: &Arc<Table>, table: &Arc<Table>) -> bool {
    let children: Vec<Arc<Table>> = lock(&of.relations)
        .iter()
        .filter_map(|r| r.child.upgrade())
        .collect();
    children
        .iter()
        .any(|child| Arc::ptr_eq(child, table) || descends(child, table))
}

/// Clears the relations of the table `parent` (`AdsClearRelation`); its
/// children stay where they are.
pub fn clear(session: &Session, parent: Handle) -> Result<(), Error> {
    let parent = table::get(session, parent)?;
    let relations = std::mem::take(&mut *lock(&parent.relations));
    for relation in relations {
        if let Some(child) = relation.child.upgrade() {
            let mut following = lock(&child.following);
            if following
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &relation))
            {
                *following = None;
            }
        }
    }
    Ok(())
}

/// Tells the children of `parent`'s relations that it moved, and their
/// descendants, which move with them.
pub(crate) fn moved(parent: &Table) {
    let relations = lock(&parent.relations).clone();
    for relation in relations {
        if let Some(child) = relation.child.upgrade() {
            *lock(&child.following) = Some(relation);
            moved(&child);
        }
    }
}

/// Moves `child` to where the relation it is to follow puts it, if its
/// parent moved since it last followed: after the parent has followed its
/// own parent.
pub(crate) fn follow(child: &Table) {
    let Some(relation) = lock(&child.following).clone() else {
        return;
    };
    let (Some(parent), Some(order)) = (relation.parent.upgrade(), relation.order.upgrade()) else {
        *lock(&child.following) = None;
        return;
    };
    parent.position();
    let Some(relation) = lock(&child.following).take() else {
        return;
    };
    let _ = follow_to(child, &parent, &order, &relation);
}

fn follow_to(
    child: &Table,
    parent: &Table,
    order: &Index,
    relation: &Relation,
) -> Result<(), Error> {
    let on_record = parent.position().record.is_some();
    let evaluation = match on_record {
        true => Some(parent.eval(&relation.expression)?),
        false => None,
    };
    let Some((value, ty)) = evaluation.and_then(|e| e.value.ok().map(|value| (value, e.ty))) else {
        // Off the parent's records: the child at EOF, its found flag kept.
        return child.go_past_end();
    };
    let text;
    let bytes;
    let sought = match &value {
        tflip_expr::Val::Str(b) => {
            text = tflip_storage::cp1252::decode(b);
            SeekValue::Text(&text)
        }
        // A negative number is not found in an NTX order, as in the original
        // engine (`fixtures/write/relations_dbf`): taken here as its text,
        // without the digits NTX keys store negative numbers with.
        tflip_expr::Val::Number(n) if *n < 0.0 && order.kind == Kind::Ntx => {
            let length = usize::from(order.tag.key_length);
            let decimals = usize::from(order.tag.decimals);
            bytes = format!("{n:>length$.decimals$}").into_bytes();
            SeekValue::Raw(&bytes)
        }
        tflip_expr::Val::Number(n) => SeekValue::Double(*n),
        tflip_expr::Val::Date(julian) => SeekValue::Double(f64::from(*julian)),
        tflip_expr::Val::Logical(b) => SeekValue::Text(if *b { "T" } else { "F" }),
        other => {
            bytes = other.bytes(ty).unwrap_or_default();
            SeekValue::Raw(&bytes)
        }
    };
    if relation.scoped {
        child.set_scope(order, false, sought)?;
        child.set_scope(order, true, sought)?;
    }
    child.seek_in(order, sought, SeekType::Hard).map(drop)
}
