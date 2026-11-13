//! The order the triggers of one timing and priority fire in
//! (`fixtures/sql/trigorder.json`).
//!
//! It is not the order of the dictionary's records and not the order the
//! triggers were made in, but the newest one first, with one exception that
//! the original's copy of the dictionary makes: a trigger made in the record
//! of an object that was in the dictionary when the connection read it, or of
//! a deleted one, stands where that record is and not at the end, so that the
//! first triggers made after the dictionary's own five were dropped fire in
//! the order they were made in, and the next ones in the opposite order. A
//! trigger that is dropped leaves no place behind but one that was in the
//! dictionary when it was read.
//!
//! tflip keeps the places as the connection's copy of the dictionary does:
//! the triggers by record (`base`), the ones made later after them (`tail`),
//! and the records whose place is free (`places`). A connection that finds
//! the dictionary changed by another one starts again from the file.

use std::collections::BTreeSet;

use tflip_storage::dd::{Dictionary, Kind};

#[derive(Debug, Clone, Copy)]
struct Entry {
    id: u32,
    record: u32,
    /// Whether it was in the dictionary when it was read (and leaves its
    /// place when it is dropped).
    loaded: bool,
}

#[derive(Debug, Default)]
pub(crate) struct TriggerOrder {
    started: bool,
    /// The triggers standing in the place of their record, by record.
    base: Vec<Entry>,
    /// The others, in the order they were made.
    tail: Vec<Entry>,
    /// The records of deleted objects whose place a new trigger takes.
    places: BTreeSet<u32>,
}

impl TriggerOrder {
    /// Gives up the places of deleted records: the first time a trigger
    /// fires the copy is rebuilt without them (the fifth trigger made after
    /// the dictionary's five were dropped stands in the place of the fifth;
    /// the ones made after a trigger has fired do not, `trigorder.json`).
    pub(crate) fn fired(&mut self) {
        self.places.clear();
    }

    /// Reads the dictionary again from the next call on.
    pub(crate) fn forget(&mut self) {
        *self = TriggerOrder::default();
    }

    /// Takes the trigger `id` out and puts it in again where it was made:
    /// one that was in the dictionary when it was read keeps its place, the
    /// others go to the end.
    pub(crate) fn altered(&mut self, id: u32) {
        if let Some(at) = self.base.iter().position(|e| e.id == id) {
            if !self.base[at].loaded {
                let entry = self.base.remove(at);
                self.tail.push(entry);
            }
        } else if let Some(at) = self.tail.iter().position(|e| e.id == id) {
            let entry = self.tail.remove(at);
            self.tail.push(entry);
        }
    }

    /// The ids of the triggers of `dict` in the order they fire in, after
    /// taking in what changed since the last call.
    pub(crate) fn sequence(&mut self, dict: &Dictionary) -> Vec<u32> {
        let triggers = dict.all(Kind::Trigger);
        if !self.started {
            self.started = true;
            self.base = triggers
                .iter()
                .map(|t| Entry {
                    id: t.id,
                    record: t.record(),
                    loaded: true,
                })
                .collect();
            let used: BTreeSet<u32> = dict.objects().iter().map(|o| o.record()).collect();
            self.places = (1..=dict.record_slots())
                .filter(|record| !used.contains(record))
                .collect();
        } else {
            let live = |id: u32| triggers.iter().any(|t| t.id == id);
            let places = &mut self.places;
            self.base.retain(|e| {
                let kept = live(e.id);
                if !kept && e.loaded {
                    places.insert(e.record);
                }
                kept
            });
            self.tail.retain(|e| live(e.id));
            let mut made: Vec<_> = triggers
                .iter()
                .filter(|t| {
                    ![&self.base, &self.tail]
                        .iter()
                        .any(|list| list.iter().any(|e| e.id == t.id))
                })
                .collect();
            made.sort_by_key(|t| t.id);
            for trigger in made {
                let entry = Entry {
                    id: trigger.id,
                    record: trigger.record(),
                    loaded: false,
                };
                if self.places.remove(&entry.record) {
                    let at = self.base.partition_point(|e| e.record < entry.record);
                    self.base.insert(at, entry);
                } else {
                    self.tail.push(entry);
                }
            }
        }
        self.tail
            .iter()
            .rev()
            .chain(self.base.iter().rev())
            .map(|e| e.id)
            .collect()
    }
}
