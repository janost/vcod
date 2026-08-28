//! Struct and array storage for script values.
//!
//! No garbage collection: a script allocates a bounded number of structs
//! and arrays over a map's lifetime, and the whole heap is dropped when the
//! map changes. Anything requiring reclamation mid-map is a later project.

use std::collections::{BTreeMap, HashMap};

use crate::atom::Atom;
use crate::value::{ArrayId, StructId, Value};

/// An array index: gsc arrays are associative, keyed by either a string
/// (`a["allies"]`) or an integer (`a[0]`). Ordered so iteration over an
/// array is deterministic.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ArrayKey {
    Int(i32),
    Str(Atom),
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
