//! Struct and array storage for script values.
//!
//! No garbage collection: a script allocates a bounded number of structs
//! and arrays over a map's lifetime, and the whole heap is dropped when the
//! map changes. Anything requiring reclamation mid-map is a later project.

use std::collections::BTreeMap;

use crate::fxhash::FxHashMap;

use crate::atom::Atom;
use crate::value::{ArrayId, StructId, Value};

/// An array index: gsc arrays are associative, keyed by either a string
/// (`a["allies"]`) or an integer (`a[0]`). Ordered so iteration over an
/// array is deterministic. The derived `Ord` puts every `Int` before every
/// `Str`, and orders `Str(Atom)` by interning order, not lexicographically
/// by the text it names — deterministic, which is all iteration needs
/// today, but it will not match retail's enumeration order once a
/// key-listing builtin (e.g. `getarraykeys`) exists.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ArrayKey {
    Int(i32),
    Str(Atom),
}

/// One array's elements: the int keys 0, 1, 2, ... in a vector while they
/// run unbroken from 0, every other key in a `BTreeMap`. Most script arrays
/// are lists, and the spawn wave reads and copies them in its inner loops
/// (cod4-server-frame.md 9, "The spawn wave"). Walked in `ArrayKey` order:
/// the negative ints, the vector, then the rest. `sparse` never holds an
/// int key in `0..=dense.len()`.
#[derive(Clone, Default)]
struct Elems {
    dense: Vec<Value>,
    sparse: BTreeMap<ArrayKey, Value>,
}

impl Elems {
    fn dense_index(&self, k: ArrayKey) -> Option<usize> {
        match k {
            ArrayKey::Int(i) if i >= 0 && (i as usize) < self.dense.len() => Some(i as usize),
            _ => None,
        }
    }

    fn get(&self, k: ArrayKey) -> Option<Value> {
        match self.dense_index(k) {
            Some(i) => Some(self.dense[i]),
            None if self.sparse.is_empty() => None,
            None => self.sparse.get(&k).copied(),
        }
    }

    /// Whether `k` is new.
    fn insert(&mut self, k: ArrayKey, v: Value) -> bool {
        if let Some(i) = self.dense_index(k) {
            self.dense[i] = v;
            return false;
        }
        if k != ArrayKey::Int(self.dense.len() as i32) || self.dense.len() >= i32::MAX as usize {
            return self.sparse.insert(k, v).is_none();
        }
        self.dense.push(v);
        // The keys past it that now run on from the vector join it.
        while !self.sparse.is_empty() {
            let next = ArrayKey::Int(self.dense.len() as i32);
            match self.sparse.remove(&next) {
                Some(v) => self.dense.push(v),
                None => break,
            }
        }
        true
    }

    /// Whether `k` was there.
    fn remove(&mut self, k: ArrayKey) -> bool {
        let Some(i) = self.dense_index(k) else {
            return self.sparse.remove(&k).is_some();
        };
        // The keys after it no longer run unbroken from 0.
        for (j, v) in self.dense.drain(i + 1..).enumerate() {
            self.sparse.insert(ArrayKey::Int((i + 1 + j) as i32), v);
        }
        self.dense.pop();
        true
    }

    fn len(&self) -> usize {
        self.dense.len() + self.sparse.len()
    }

    /// Every element in `ArrayKey` order.
    fn iter(&self) -> impl Iterator<Item = (ArrayKey, Value)> + '_ {
        let below = self.sparse.range(..ArrayKey::Int(0));
        let above = self.sparse.range(ArrayKey::Int(0)..);
        below
            .map(|(k, v)| (*k, *v))
            .chain(
                self.dense
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (ArrayKey::Int(i as i32), *v)),
            )
            .chain(above.map(|(k, v)| (*k, *v)))
    }
}

#[derive(Default)]
pub struct Heap {
    structs: Vec<FxHashMap<Atom, Value>>,
    arrays: Vec<Elems>,
    /// Each array's keys in the order they were first written, for
    /// [`Heap::array_keys_newest_first`].
    orders: Vec<Vec<ArrayKey>>,
    /// Arrays a second place may hold ([`Heap::share`]): a write through
    /// one copies it first ([`Heap::own`]).
    shared: Vec<bool>,
}

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_struct(&mut self) -> StructId {
        let id = StructId(self.structs.len() as u32);
        self.structs.push(FxHashMap::default());
        id
    }

    pub fn new_array(&mut self) -> ArrayId {
        let id = ArrayId(self.arrays.len() as u32);
        self.arrays.push(Elems::default());
        self.orders.push(Vec::new());
        self.shared.push(false);
        id
    }

    /// An array value stored somewhere new (a variable, a field, an
    /// element, an argument): from here on two places may hold it. Retail
    /// counts the references (`Scr_EvalArrayRef`'s `refCount`); vcod keeps
    /// the flag for good, which only costs a copy nobody could tell apart.
    pub fn share(&mut self, v: Value) {
        if let Value::Array(a) = v
            && let Some(f) = self.shared.get_mut(a.0 as usize)
        {
            *f = true;
        }
    }

    /// The array a write through `a` goes to: `a` itself when nothing else
    /// holds it, else a fresh copy the caller stores back where it read
    /// `a`. gsc arrays are values: `b = a; b[1] = x;` leaves `a` alone, on
    /// every engine (`probe_array_copy`, 1.1d; `CopyArray` under
    /// `Scr_EvalArrayRef`, cod4-gsc.md 10). The copy is deep as retail's is,
    /// lazily: the arrays it holds are now held twice, so they are shared.
    pub fn own(&mut self, a: ArrayId) -> ArrayId {
        let i = a.0 as usize;
        if !self.shared.get(i).copied().unwrap_or(false) {
            return a;
        }
        let elems = self.arrays[i].clone();
        let order = self.orders[i].clone();
        for (_, v) in elems.iter() {
            self.share(v);
        }
        let id = ArrayId(self.arrays.len() as u32);
        self.arrays.push(elems);
        self.orders.push(order);
        self.shared.push(false);
        id
    }

    /// Reading an unset field yields `Undefined`, matching gsc semantics; so
    /// does reading a struct id out of range, since an embedder can now mint
    /// one (`Vm::heap`) and a bad one must not panic.
    pub fn get_field(&self, s: StructId, f: Atom) -> Value {
        self.structs
            .get(s.0 as usize)
            .and_then(|fields| fields.get(&f))
            .copied()
            .unwrap_or(Value::Undefined)
    }

    /// A struct id out of range is silently dropped rather than panicking,
    /// for the same reason as `get_field`.
    pub fn set_field(&mut self, s: StructId, f: Atom, v: Value) {
        if let Some(fields) = self.structs.get_mut(s.0 as usize) {
            fields.insert(f, v);
        } else {
            log::warn!("dropped a write to struct {} (out of range)", s.0);
        }
    }

    /// Reading an unset key, or an array id out of range, yields `Undefined`.
    pub fn get_index(&self, a: ArrayId, k: ArrayKey) -> Value {
        self.arrays
            .get(a.0 as usize)
            .and_then(|elems| elems.get(k))
            .unwrap_or(Value::Undefined)
    }

    /// Writing an unset key grows the array, `undefined` included; there is
    /// no fixed size. An array id out of range is silently
    /// dropped rather than panicking.
    pub fn set_index(&mut self, a: ArrayId, k: ArrayKey, v: Value) {
        if let Some(elems) = self.arrays.get_mut(a.0 as usize) {
            if elems.insert(k, v) {
                self.orders[a.0 as usize].push(k);
            }
        } else {
            log::warn!("dropped a write to array {} (out of range)", a.0);
        }
    }

    /// `a[k] = undefined` with the literal: the key goes, so `.size` drops,
    /// the stock `a[a.size - 1] = undefined` shrink (`probe_array_undef`).
    /// An undefined value from anywhere else is stored by [`Self::set_index`]
    /// and counted (`probe_undef_call_into_unset`).
    pub fn clear_index(&mut self, a: ArrayId, k: ArrayKey) {
        if let Some(elems) = self.arrays.get_mut(a.0 as usize)
            && elems.remove(k)
        {
            let order = &mut self.orders[a.0 as usize];
            if let Some(i) = order.iter().rposition(|o| *o == k) {
                order.remove(i);
            }
        }
    }

    /// An array's element count, for `.size` and for a host that walks one.
    /// An id out of range reads as 0, the same no-panic rule as the accessors.
    pub fn array_len(&self, a: ArrayId) -> usize {
        self.arrays.get(a.0 as usize).map_or(0, |e| e.len())
    }

    /// Every key and value in an array, in `ArrayKey` order. For a caller
    /// that has to walk one from outside the instruction loop; an id out of
    /// range yields nothing.
    pub fn array_entries(&self, a: ArrayId) -> Vec<(ArrayKey, Value)> {
        self.arrays
            .get(a.0 as usize)
            .map(|e| e.iter().collect())
            .unwrap_or_default()
    }

    /// An array's keys, the most recently added first (CoD4's
    /// `getArrayKeys`, docs/research/cod4-gsc.md 3); an id out of range
    /// yields nothing.
    pub fn array_keys_newest_first(&self, a: ArrayId) -> Vec<ArrayKey> {
        self.orders
            .get(a.0 as usize)
            .map(|o| o.iter().rev().copied().collect())
            .unwrap_or_default()
    }

    /// Every field name and value on a struct. The order is the `HashMap`'s,
    /// so a caller that needs a stable one sorts; nothing reads a struct back
    /// in script order.
    pub fn struct_fields(&self, s: StructId) -> Vec<(Atom, Value)> {
        self.structs
            .get(s.0 as usize)
            .map(|f| f.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default()
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
        let f = i.intern_folded("hp");
        assert_eq!(h.get_field(s, f), Value::Undefined);
    }

    #[test]
    fn a_struct_field_round_trips_through_set_and_get() {
        let mut h = Heap::new();
        let mut i = Interner::default();
        let s = h.new_struct();
        let f = i.intern_folded("hp");
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

    /// The vector of keys run from 0 and the map of the rest read and walk
    /// as one map would: every write order, gaps filled and keys cleared.
    #[test]
    fn an_array_reads_and_walks_as_one_map_whatever_the_write_order() {
        let mut i = Interner::default();
        let s = i.intern_exact("s");
        let keys = [
            ArrayKey::Int(2),
            ArrayKey::Int(0),
            ArrayKey::Str(s),
            ArrayKey::Int(-1),
            ArrayKey::Int(1),
            ArrayKey::Int(5),
            ArrayKey::Int(3),
            ArrayKey::Int(4),
        ];
        for clear in [None, Some(ArrayKey::Int(1)), Some(ArrayKey::Int(5))] {
            let mut h = Heap::new();
            let a = h.new_array();
            let mut want = BTreeMap::new();
            for (n, k) in keys.iter().enumerate() {
                h.set_index(a, *k, Value::Int(n as i32));
                want.insert(*k, Value::Int(n as i32));
            }
            if let Some(k) = clear {
                h.clear_index(a, k);
                want.remove(&k);
            }
            h.set_index(a, ArrayKey::Int(1), Value::Int(99));
            want.insert(ArrayKey::Int(1), Value::Int(99));
            let got = h.array_entries(a);
            let want: Vec<_> = want.into_iter().collect();
            assert_eq!(got, want);
            assert_eq!(h.array_len(a), want.len());
            for (k, v) in &want {
                assert_eq!(h.get_index(a, *k), *v);
            }
            assert_eq!(h.get_index(a, ArrayKey::Int(7)), Value::Undefined);
        }
    }

    /// A `StructId`/`ArrayId` can now be minted outside this crate (`Vm::heap`),
    /// so an out-of-range one from a bug on the embedder's side must read
    /// and write as a no-op, not panic.
    #[test]
    fn an_out_of_range_id_reads_undefined_and_a_write_is_a_no_op() {
        let mut h = Heap::new();
        let mut i = Interner::default();
        let f = i.intern_folded("hp");
        let bad_struct = StructId(99);
        let bad_array = ArrayId(99);
        assert_eq!(h.get_field(bad_struct, f), Value::Undefined);
        h.set_field(bad_struct, f, Value::Int(1));
        assert_eq!(h.get_index(bad_array, ArrayKey::Int(0)), Value::Undefined);
        h.set_index(bad_array, ArrayKey::Int(0), Value::Int(1));
    }

    /// New structs and arrays get distinct ids off the same heap, and
    /// writing one never leaks into another.
    #[test]
    fn distinct_structs_do_not_share_storage() {
        let mut h = Heap::new();
        let mut i = Interner::default();
        let a = h.new_struct();
        let b = h.new_struct();
        let f = i.intern_folded("hp");
        h.set_field(a, f, Value::Int(1));
        assert_eq!(h.get_field(b, f), Value::Undefined);
    }
}
