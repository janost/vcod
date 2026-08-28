//! Struct and array storage for script values.
//!
//! No garbage collection: a script allocates a bounded number of structs
//! and arrays over a map's lifetime, and the whole heap is dropped when the
//! map changes. Anything requiring reclamation mid-map is a later project.

use std::collections::{BTreeMap, HashMap};

use crate::atom::{Atom, StrAtom};
use crate::value::{ArrayId, StructId, Value};

/// An array index: gsc arrays are associative, keyed by either a string
/// (`a["allies"]`) or an integer (`a[0]`). Ordered so iteration over an
/// array is deterministic. The derived `Ord` puts every `Int` before every
/// `Str`, and orders `Str(StrAtom)` by interning order, not lexicographically
/// by the text it names — deterministic, which is all iteration needs
/// today, but it will not match retail's enumeration order once a
/// key-listing builtin (e.g. `getarraykeys`) exists.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ArrayKey {
    Int(i32),
    Str(StrAtom),
}

#[derive(Default)]
pub struct Heap {
    structs: Vec<HashMap<Atom, Value>>,
    arrays: Vec<BTreeMap<ArrayKey, Value>>,
}

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_struct(&mut self) -> StructId {
        let id = StructId(self.structs.len() as u32);
        self.structs.push(HashMap::new());
        id
    }

    pub fn new_array(&mut self) -> ArrayId {
        let id = ArrayId(self.arrays.len() as u32);
        self.arrays.push(BTreeMap::new());
        id
    }

    /// Reading an unset field yields `Undefined`, matching gsc semantics.
    pub fn get_field(&self, s: StructId, f: Atom) -> Value {
        self.structs[s.0 as usize]
            .get(&f)
            .copied()
            .unwrap_or(Value::Undefined)
    }

    pub fn set_field(&mut self, s: StructId, f: Atom, v: Value) {
        self.structs[s.0 as usize].insert(f, v);
    }

    /// Reading an unset key yields `Undefined`.
    pub fn get_index(&self, a: ArrayId, k: ArrayKey) -> Value {
        self.arrays[a.0 as usize]
            .get(&k)
            .copied()
            .unwrap_or(Value::Undefined)
    }

    /// Writing an unset key grows the array; there is no fixed size.
    pub fn set_index(&mut self, a: ArrayId, k: ArrayKey, v: Value) {
        self.arrays[a.0 as usize].insert(k, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::Interner;

    #[test]
    fn reading_an_unset_struct_field_is_undefined() {
        let mut h = Heap::new();
        let mut i = Interner::default();
        let s = h.new_struct();
        let f = i.intern("hp");
        assert_eq!(h.get_field(s, f), Value::Undefined);
    }

    #[test]
    fn a_struct_field_round_trips_through_set_and_get() {
        let mut h = Heap::new();
        let mut i = Interner::default();
        let s = h.new_struct();
        let f = i.intern("hp");
        h.set_field(s, f, Value::Int(100));
        assert_eq!(h.get_field(s, f), Value::Int(100));
    }

    #[test]
    fn reading_an_unset_array_index_is_undefined() {
        let mut h = Heap::new();
        let a = h.new_array();
        assert_eq!(h.get_index(a, ArrayKey::Int(0)), Value::Undefined);
    }

    #[test]
    fn an_array_index_round_trips_through_set_and_get() {
        let mut h = Heap::new();
        let a = h.new_array();
        h.set_index(a, ArrayKey::Int(3), Value::Int(7));
        assert_eq!(h.get_index(a, ArrayKey::Int(3)), Value::Int(7));
    }

    /// New structs and arrays get distinct ids off the same heap, and
    /// writing one never leaks into another.
    #[test]
    fn distinct_structs_do_not_share_storage() {
        let mut h = Heap::new();
        let mut i = Interner::default();
        let a = h.new_struct();
        let b = h.new_struct();
        let f = i.intern("hp");
        h.set_field(a, f, Value::Int(1));
        assert_eq!(h.get_field(b, f), Value::Undefined);
    }
}
