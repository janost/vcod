//! Struct and array storage for script values.
//!
//! Retail frees a variable when its reference count drops to zero. vcod's
//! values are plain ids, so the heap is swept instead: between frames the VM
//! marks what its threads, its globals and the host still hold
//! ([`Heap::collect`]) and frees the rest for the next allocation to reuse.
//! [`Heap::vars`] counts what is allocated, and the VM ends a thread that
//! takes it past its ceiling (`interp::MAX_VARIABLES`), as retail's variable
//! pool runs out.

use std::collections::BTreeMap;

use crate::fxhash::FxHashMap;

use crate::atom::Atom;
use crate::value::{ArrayId, StructId, Value};

/// An array index: gsc arrays are associative, keyed by either a string
/// (`a["allies"]`) or an integer (`a[0]`). Ordered so iteration over an
/// array is deterministic. The derived `Ord` puts every `Int` before every
/// `Str`, and orders `Str(Atom)` by interning order, not lexicographically
/// by the text it names: deterministic, which is all iteration needs.
/// `getArrayKeys` does not read this order; it takes insertion order
/// ([`Heap::array_keys_newest_first`]).
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
    /// The keys in the order they were first written, for
    /// [`Heap::array_keys_newest_first`]; `None` while that is
    /// `0..dense.len()` (each key appended in turn, none cleared but the
    /// last), which a list never leaves and so never allocates. `None`
    /// implies `sparse` is empty.
    order: Option<Vec<ArrayKey>>,
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

    /// The first-write order, written out.
    fn order_mut(&mut self) -> &mut Vec<ArrayKey> {
        let n = self.dense.len();
        self.order
            .get_or_insert_with(|| (0..n).map(|i| ArrayKey::Int(i as i32)).collect())
    }

    /// Whether `k` is new.
    fn insert(&mut self, k: ArrayKey, v: Value) -> bool {
        if let Some(i) = self.dense_index(k) {
            self.dense[i] = v;
            return false;
        }
        if k != ArrayKey::Int(self.dense.len() as i32) || self.dense.len() >= i32::MAX as usize {
            let new = self.sparse.insert(k, v).is_none();
            if new {
                self.order_mut().push(k);
            }
            return new;
        }
        if let Some(order) = self.order.as_mut() {
            order.push(k);
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
        let last = self.dense.len().checked_sub(1);
        match self.dense_index(k) {
            Some(i) if self.order.is_none() && Some(i) == last => {}
            Some(_) => self.drop_from_order(k),
            None => {
                if self.sparse.remove(&k).is_none() {
                    return false;
                }
                self.drop_from_order(k);
                return true;
            }
        }
        let i = self.dense_index(k).expect("a dense key");
        // The keys after it no longer run unbroken from 0.
        for (j, v) in self.dense.drain(i + 1..).enumerate() {
            self.sparse.insert(ArrayKey::Int((i + 1 + j) as i32), v);
        }
        self.dense.pop();
        true
    }

    fn drop_from_order(&mut self, k: ArrayKey) {
        let order = self.order_mut();
        if let Some(i) = order.iter().rposition(|o| *o == k) {
            order.remove(i);
        }
    }

    fn len(&self) -> usize {
        self.dense.len() + self.sparse.len()
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        self.dense.iter_mut().chain(self.sparse.values_mut())
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

    /// The keys, the most recently added first.
    fn keys_newest_first(&self) -> Vec<ArrayKey> {
        match &self.order {
            Some(o) => o.iter().rev().copied().collect(),
            None => (0..self.dense.len())
                .rev()
                .map(|i| ArrayKey::Int(i as i32))
                .collect(),
        }
    }
}

/// How many objects may be allocated past the last sweep's survivors before
/// [`Heap::collect_due`] asks for the next: a sweep costs a walk of the live
/// heap, so it waits for as many new objects as survived, and for at least
/// this many: about two and a half minutes of stock CoD4 `war` with 12 bots
/// (cod2-gsc-diff.md 3.7).
const COLLECT_FLOOR: usize = 16_384;

#[derive(Default)]
pub struct Heap {
    structs: Vec<FxHashMap<Atom, Value>>,
    arrays: Vec<Elems>,
    /// Arrays a second place may hold ([`Heap::share`]): a write through
    /// one copies it first ([`Heap::own`]).
    shared: Vec<bool>,
    /// The slots the last sweep freed, the lowest last: what the next
    /// allocations take.
    free_structs: Vec<u32>,
    free_arrays: Vec<u32>,
    /// Objects allocated since the last sweep, and how many it kept.
    allocated: usize,
    survivors: usize,
    /// [`Heap::vars`].
    vars: u64,
}

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_struct(&mut self) -> StructId {
        self.allocated += 1;
        self.vars += 1;
        if let Some(i) = self.free_structs.pop() {
            return StructId(i);
        }
        let id = StructId(self.structs.len() as u32);
        self.structs.push(FxHashMap::default());
        id
    }

    pub fn new_array(&mut self) -> ArrayId {
        self.alloc_array(Elems::default())
    }

    /// A slot for `elems`, a freed one first, unshared.
    fn alloc_array(&mut self, elems: Elems) -> ArrayId {
        self.allocated += 1;
        self.vars += 1 + elems.len() as u64;
        if let Some(i) = self.free_arrays.pop() {
            self.arrays[i as usize] = elems;
            return ArrayId(i);
        }
        let id = ArrayId(self.arrays.len() as u32);
        self.arrays.push(elems);
        self.shared.push(false);
        id
    }

    /// Every struct and array allocated, plus their fields and elements:
    /// what the last sweep kept and everything allocated since. One per
    /// variable retail's pool would hold for them, plus the garbage since
    /// the sweep.
    pub fn vars(&self) -> u64 {
        self.vars
    }

    /// The array and the struct slots, the freed ones included: the heap's
    /// footprint.
    pub fn objects(&self) -> (usize, usize) {
        (self.arrays.len(), self.structs.len())
    }

    /// Whether enough has been allocated since the last sweep for the VM
    /// to run the next ([`COLLECT_FLOOR`]).
    pub(crate) fn collect_due(&self) -> bool {
        self.allocated >= self.survivors.max(COLLECT_FLOOR)
    }

    /// Frees every struct and array `roots` cannot reach, at any depth, for
    /// the next allocations to reuse, and recounts [`Heap::vars`] over what
    /// is left. `roots` must name every value held outside the heap: a
    /// thread's locals and operand stack, the globals, and the host's. The
    /// same walk counts each array's holders, retail's `refCount` plus one,
    /// so an array one place holds is no longer [`Heap::share`]d and its
    /// next write goes to it in place, as retail's does.
    pub fn collect(&mut self, roots: impl IntoIterator<Item = Value>) {
        let mut struct_live = vec![false; self.structs.len()];
        let mut holders = vec![0u8; self.arrays.len()];
        let mut todo = Vec::new();
        let mut visit = |v: Value, todo: &mut Vec<Value>| match v {
            Value::Array(a) => {
                if let Some(n) = holders.get_mut(a.0 as usize) {
                    *n = n.saturating_add(1);
                    if *n == 1 {
                        todo.push(v);
                    }
                }
            }
            Value::Struct(s) => {
                if let Some(live) = struct_live.get_mut(s.0 as usize)
                    && !*live
                {
                    *live = true;
                    todo.push(v);
                }
            }
            _ => {}
        };
        for v in roots {
            visit(v, &mut todo);
        }
        while let Some(v) = todo.pop() {
            match v {
                Value::Array(a) => {
                    for (_, e) in self.arrays[a.0 as usize].iter() {
                        visit(e, &mut todo);
                    }
                }
                Value::Struct(s) => {
                    for &e in self.structs[s.0 as usize].values() {
                        visit(e, &mut todo);
                    }
                }
                _ => {}
            }
        }
        self.vars = 0;
        self.survivors = 0;
        self.free_arrays.clear();
        for (i, n) in holders.iter().enumerate().rev() {
            if *n == 0 {
                self.arrays[i] = Elems::default();
                self.free_arrays.push(i as u32);
            } else {
                self.vars += 1 + self.arrays[i].len() as u64;
                self.survivors += 1;
            }
            self.shared[i] = *n > 1;
        }
        self.free_structs.clear();
        for (i, live) in struct_live.iter().enumerate().rev() {
            if *live {
                self.vars += 1 + self.structs[i].len() as u64;
                self.survivors += 1;
            } else {
                self.structs[i] = FxHashMap::default();
                self.free_structs.push(i as u32);
            }
        }
        self.allocated = 0;
    }

    /// Whether `to` is `from` or an array `from` holds, at any depth through
    /// arrays.
    pub fn reaches(&self, from: ArrayId, to: ArrayId) -> bool {
        if from == to {
            return true;
        }
        let mut seen = std::collections::HashSet::<u32, crate::fxhash::FxBuildHasher>::default();
        let mut todo = vec![from];
        while let Some(a) = todo.pop() {
            let Some(elems) = self.arrays.get(a.0 as usize) else {
                continue;
            };
            for (_, v) in elems.iter() {
                if let Value::Array(b) = v {
                    if b == to {
                        return true;
                    }
                    if seen.insert(b.0) {
                        todo.push(b);
                    }
                }
            }
        }
        false
    }

    /// A copy of `a` and of every array it holds, at any depth, none of them
    /// sharing storage with what they copy; structs and other values are
    /// held as they are. What `a[0] = a` stores: the array as it stood,
    /// not a cycle through it.
    pub fn snapshot(&mut self, a: ArrayId) -> ArrayId {
        let root = self.new_array();
        let mut copies = FxHashMap::default();
        copies.insert(a.0, root);
        let mut todo = vec![(a, root)];
        while let Some((src, dst)) = todo.pop() {
            let mut elems = self.arrays.get(src.0 as usize).cloned().unwrap_or_default();
            for v in elems.values_mut() {
                if let Value::Array(b) = *v {
                    let copy = match copies.get(&b.0) {
                        Some(&c) => c,
                        None => {
                            let c = self.new_array();
                            copies.insert(b.0, c);
                            todo.push((b, c));
                            c
                        }
                    };
                    // A copy two elements hold is shared, as `own`'s are.
                    self.shared[copy.0 as usize] = true;
                    *v = Value::Array(copy);
                }
            }
            self.vars += elems.len() as u64;
            self.arrays[dst.0 as usize] = elems;
        }
        root
    }

    /// An array value stored somewhere new (a variable, a field, an
    /// element, an argument): from here on two places may hold it. Retail
    /// counts the references (`Scr_EvalArrayRef`'s `refCount`); vcod keeps
    /// the flag until the next sweep counts the holders ([`Heap::collect`]),
    /// which only costs a copy nobody could tell apart.
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
        // Room for one more: the copy is most often a write's
        // (`add_to_array`'s append).
        let src = &self.arrays[i];
        let mut dense = Vec::with_capacity(src.dense.len() + 1);
        dense.extend_from_slice(&src.dense);
        let elems = Elems {
            dense,
            sparse: src.sparse.clone(),
            order: src.order.clone(),
        };
        for (_, v) in elems.iter() {
            self.share(v);
        }
        self.alloc_array(elems)
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
            if fields.insert(f, v).is_none() {
                self.vars += 1;
            }
        } else {
            log::warn!("dropped a write to struct {} (out of range)", s.0);
        }
    }

    /// `f` as an undefined field unless `s` has one (`Cx::touch_field`).
    pub fn touch_field(&mut self, s: StructId, f: Atom) {
        if let Some(fields) = self.structs.get_mut(s.0 as usize)
            && let std::collections::hash_map::Entry::Vacant(e) = fields.entry(f)
        {
            e.insert(Value::Undefined);
            self.vars += 1;
        }
    }

    /// Removes `f` from `s` (`Cx::clear_field`).
    pub fn clear_field(&mut self, s: StructId, f: Atom) {
        if let Some(fields) = self.structs.get_mut(s.0 as usize)
            && fields.remove(&f).is_some()
        {
            self.vars -= 1;
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
                self.vars += 1;
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
            self.vars -= 1;
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
        self.arrays
            .get(a.0 as usize)
            .map(Elems::keys_newest_first)
            .unwrap_or_default()
    }

    /// How many struct fields and array elements hold each array, by id:
    /// the heap's share of retail's `refCount` plus one, which
    /// `scriptUsage` divides an array's usage by (`Vm::thread_usage`).
    /// Garbage since the last sweep counts too.
    pub fn array_holders(&self) -> FxHashMap<u32, u32> {
        let mut n = FxHashMap::default();
        let values = self
            .structs
            .iter()
            .flat_map(|f| f.values().copied())
            .chain(self.arrays.iter().flat_map(|e| e.iter().map(|(_, v)| v)));
        for v in values {
            if let Value::Array(a) = v {
                *n.entry(a.0).or_insert(0) += 1;
            }
        }
        n
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
            let mut order: Vec<ArrayKey> = keys.to_vec();
            order.retain(|k| Some(*k) != clear);
            if clear == Some(ArrayKey::Int(1)) {
                order.push(ArrayKey::Int(1));
            }
            order.reverse();
            assert_eq!(h.array_keys_newest_first(a), order);
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

    /// A list's keys come back newest first without a written order, and
    /// clearing its last key keeps it a list.
    #[test]
    fn a_list_lists_its_keys_newest_first() {
        let mut h = Heap::new();
        let a = h.new_array();
        for i in 0..4 {
            h.set_index(a, ArrayKey::Int(i), Value::Int(i));
        }
        h.clear_index(a, ArrayKey::Int(3));
        h.set_index(a, ArrayKey::Int(3), Value::Int(9));
        let keys: Vec<_> = (0..4).rev().map(ArrayKey::Int).collect();
        assert_eq!(h.array_keys_newest_first(a), keys);
        h.clear_index(a, ArrayKey::Int(1));
        h.set_index(a, ArrayKey::Int(1), Value::Int(8));
        let keys = [1, 3, 2, 0].map(ArrayKey::Int);
        assert_eq!(h.array_keys_newest_first(a), keys);
        assert_eq!(h.array_len(a), 4);
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
