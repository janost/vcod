//! Interned script strings; `Atom` equality is a `u32` compare.
//!
//! Two entry points over one storage vector, because retail folds case for
//! identifier-role names and not for string values or array keys.
//! `intern_folded` dedups on the lowercased text, `intern_exact` on the
//! exact text; both store the spelling they were given verbatim, and
//! `resolve` returns it. Which role takes which, the measurements behind
//! the split and the corpus counts are in
//! docs/research/cod11-gsc-language.md, "Atom identity and case folding".
//!
//! The invariant everything else leans on: whichever entry point sees a
//! folded key first owns it, so the two can never hand out different atoms
//! for one identifier, and `fold_atom` maps any atom onto that owner. Event
//! names reach the VM as case-preserved string values, so `Op::WaitTill`,
//! `Op::EndOn` and `Vm::notify` fold before matching: if one stopped, a
//! waiting thread would hang with nothing logged.
//!
//! A string the VM builds at run time (`s = s + "x"`) takes a *temporary*
//! atom (`intern_temp`), which a sweep between frames frees once no script
//! value names it (`Interner::sweep`, from `Vm::collect`). Every other
//! atom is permanent: the compiler's, and every one the host interns or is
//! handed. A temporary atom becomes permanent the moment it reaches the
//! host (`Interner::pin`: a builtin's argument, an entity field write, a
//! value read through `Cx`), so an `Atom` a host keeps in a Rust struct or
//! keys a cache on never comes back as other text. The measurement behind
//! it is in docs/research/cod2-gsc-diff.md 3.7.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::fxhash::FxHashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Atom(pub u32);

/// What a sweep may do with an atom ([`Interner::sweep`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Kept for the level's life.
    Permanent,
    /// Built at run time and never handed to the host: freed once nothing
    /// names it.
    Temporary,
    /// Freed; the next intern takes it.
    Free,
}

pub struct Interner {
    /// [`Interner::id`].
    id: u64,
    /// Exact-text lookup, for string values and array keys.
    by_exact: FxHashMap<String, Atom>,
    /// Case-folded lookup, for identifiers, field names, file paths,
    /// function names and event names.
    by_folded: FxHashMap<String, Atom>,
    /// The spelling first interned for each atom; what `resolve` returns.
    text: Vec<String>,
    /// The case-folded form of the same atom; what `resolve_folded` returns.
    folded: Vec<String>,
    /// The atom that owns this atom's folded key; what `fold_atom` returns.
    /// A permanent atom's is permanent ([`Interner::pin`]).
    canon: Vec<Atom>,
    /// A `Cell` so a read through `&Cx` can pin what it hands out.
    kind: Vec<Cell<Kind>>,
    /// The atoms the last sweep freed, the lowest last.
    free: Vec<u32>,
    /// Temporary text interned since the last sweep, and what it kept.
    temp_new: usize,
    temp_kept: usize,
    /// A vector renders as `(%g, %g, %g)`, CoD2's `SL_GetStringForVector`,
    /// not 1.1's two decimals (`value::format_number`).
    pub vectors_g: bool,
    /// [`Interner::bytes`].
    bytes: usize,
}

/// How much temporary text may be interned past what the last sweep kept
/// before [`Interner::collect_due`] asks for the next.
const COLLECT_FLOOR: usize = 1 << 20;

/// The next [`Interner::id`].
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Default for Interner {
    fn default() -> Self {
        Interner {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            by_exact: FxHashMap::default(),
            by_folded: FxHashMap::default(),
            text: Vec::new(),
            folded: Vec::new(),
            canon: Vec::new(),
            kind: Vec::new(),
            free: Vec::new(),
            temp_new: 0,
            temp_kept: 0,
            vectors_g: false,
            bytes: 0,
        }
    }
}

/// A clone takes a new [`Interner::id`]: the two go on interning apart, so
/// one atom number may come to name two texts.
impl Clone for Interner {
    fn clone(&self) -> Self {
        Interner {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            by_exact: self.by_exact.clone(),
            by_folded: self.by_folded.clone(),
            text: self.text.clone(),
            folded: self.folded.clone(),
            canon: self.canon.clone(),
            kind: self.kind.clone(),
            free: self.free.clone(),
            temp_new: self.temp_new,
            temp_kept: self.temp_kept,
            vectors_g: self.vectors_g,
            bytes: self.bytes,
        }
    }
}

impl Interner {
    /// This interner's own number, unique in the process: a host that
    /// caches what it worked out per atom keys the cache on it, since an
    /// atom only means one text within one interner, which never takes back
    /// an atom the host was given.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// A slot for a new atom, a freed one first.
    fn alloc(&mut self, s: &str, folded: String, canon: Option<Atom>, kind: Kind) -> Atom {
        self.bytes += s.len();
        if let Some(i) = self.free.pop() {
            let a = Atom(i);
            let i = i as usize;
            self.text[i] = s.to_string();
            self.folded[i] = folded;
            self.canon[i] = canon.unwrap_or(a);
            self.kind[i].set(kind);
            return a;
        }
        let a = Atom(self.text.len() as u32);
        self.text.push(s.to_string());
        self.folded.push(folded);
        self.canon.push(canon.unwrap_or(a));
        self.kind.push(Cell::new(kind));
        a
    }

    /// An atom for `s` by exact text, with the folded key claimed if
    /// nothing holds it yet, so `intern_exact` and `intern_folded` can
    /// never disagree about which atom a folded key resolves to, whichever
    /// role saw the text first.
    fn new_exact(&mut self, s: &str, kind: Kind) -> Atom {
        let folded = s.to_ascii_lowercase();
        let owner = self.by_folded.get(&folded).copied();
        let a = self.alloc(s, folded.clone(), owner, kind);
        if owner.is_none() {
            self.by_folded.insert(folded, a);
        }
        self.by_exact.insert(s.to_string(), a);
        a
    }

    /// Interns by exact text: two spellings of one word get two atoms.
    pub fn intern_exact(&mut self, s: &str) -> Atom {
        if let Some(&a) = self.by_exact.get(s) {
            self.pin_atom(a);
            return a;
        }
        let a = self.new_exact(s, Kind::Permanent);
        self.pin_atom(self.canon[a.0 as usize]);
        a
    }

    /// [`Interner::intern_exact`] for a string the VM built at run time:
    /// temporary unless the text already has an atom ([`Interner::sweep`]).
    pub(crate) fn intern_temp(&mut self, s: &str) -> Atom {
        if let Some(&a) = self.by_exact.get(s) {
            return a;
        }
        self.temp_new += s.len();
        self.new_exact(s, Kind::Temporary)
    }

    /// Interns by case-folded text: every spelling of one word gets the
    /// atom the first spelling seen was given.
    pub fn intern_folded(&mut self, s: &str) -> Atom {
        if let Some(a) = self.lookup_folded(s) {
            self.pin_atom(a);
            return a;
        }
        let folded = s.to_ascii_lowercase();
        let a = self.alloc(s, folded.clone(), None, Kind::Permanent);
        self.by_folded.insert(folded, a);
        // Two atoms with identical text would make `"x" == "x"` false, so
        // this spelling claims the exact key as well.
        self.by_exact.entry(s.to_string()).or_insert(a);
        a
    }

    /// Looks up an exact spelling without interning it.
    pub fn get_exact(&self, s: &str) -> Option<Atom> {
        let a = self.lookup_exact(s);
        a.inspect(|&a| self.pin_atom(a))
    }

    /// [`Interner::get_exact`] without the pin, for the VM's own use.
    pub(crate) fn lookup_exact(&self, s: &str) -> Option<Atom> {
        self.by_exact.get(s).copied()
    }

    /// Looks up any spelling of a folded key without interning it.
    /// Allocates only for a spelling with an uppercase letter.
    pub fn get_folded(&self, s: &str) -> Option<Atom> {
        let a = self.lookup_folded(s);
        a.inspect(|&a| self.pin_atom(a))
    }

    fn lookup_folded(&self, s: &str) -> Option<Atom> {
        if s.bytes().any(|c| c.is_ascii_uppercase()) {
            self.by_folded.get(&s.to_ascii_lowercase()).copied()
        } else {
            self.by_folded.get(s).copied()
        }
    }

    /// The spelling first interned for this atom, verbatim.
    pub fn resolve(&self, a: Atom) -> &str {
        &self.text[a.0 as usize]
    }

    /// The case-folded form of this atom's text, for matching against a
    /// fixed lowercase literal.
    pub fn resolve_folded(&self, a: Atom) -> &str {
        &self.folded[a.0 as usize]
    }

    /// The atom `intern_folded` would return for this atom's text. Every
    /// spelling of one event name folds to the same atom through this, which
    /// is what lets a `notify` reach a `waittill` that spelled the event
    /// differently.
    pub fn fold_atom(&self, a: Atom) -> Atom {
        self.canon[a.0 as usize]
    }

    /// Makes `a`, and the atom owning its folded key, permanent.
    fn pin_atom(&self, a: Atom) {
        let i = a.0 as usize;
        if self.kind[i].get() == Kind::Temporary {
            self.kind[i].set(Kind::Permanent);
            self.pin_atom(self.canon[i]);
        }
    }

    /// Makes the atom `v` holds permanent, if it holds one: whatever the
    /// VM hands the host goes through here, so no sweep frees an atom the
    /// host may have kept.
    pub(crate) fn pin(&self, v: crate::value::Value) {
        use crate::value::Value;
        if let Value::String(a) | Value::Localized(a) | Value::Anim(a) = v {
            self.pin_atom(a);
        }
    }

    /// Whether `v` holds a temporary atom ([`Interner::intern_temp`]).
    pub(crate) fn holds_temporary(&self, v: crate::value::Value) -> bool {
        use crate::value::Value;
        match v {
            Value::String(a) | Value::Localized(a) | Value::Anim(a) => {
                self.kind[a.0 as usize].get() == Kind::Temporary
            }
            _ => false,
        }
    }

    /// Whether enough temporary text has been interned since the last
    /// sweep for the VM to run the next ([`COLLECT_FLOOR`]).
    pub(crate) fn collect_due(&self) -> bool {
        self.temp_new >= self.temp_kept.max(COLLECT_FLOOR)
    }

    /// A mark for every atom, all clear, for a sweep's walk to set.
    pub(crate) fn marks(&self) -> Vec<bool> {
        vec![false; self.text.len()]
    }

    /// Frees every temporary atom `marked` leaves clear, keeping the one
    /// owning a kept atom's folded key, so the next interns reuse them.
    pub(crate) fn sweep(&mut self, mut marked: Vec<bool>) {
        marked.resize(self.text.len(), false);
        for i in 0..self.text.len() {
            if marked[i] {
                marked[self.canon[i].0 as usize] = true;
            }
        }
        self.free.clear();
        self.temp_kept = 0;
        for i in (0..self.text.len()).rev() {
            match self.kind[i].get() {
                Kind::Temporary if marked[i] => self.temp_kept += self.text[i].len(),
                Kind::Temporary => {
                    let text = std::mem::take(&mut self.text[i]);
                    let folded = std::mem::take(&mut self.folded[i]);
                    self.bytes -= text.len();
                    let a = Atom(i as u32);
                    if self.by_exact.get(&text) == Some(&a) {
                        self.by_exact.remove(&text);
                    }
                    if self.by_folded.get(&folded) == Some(&a) {
                        self.by_folded.remove(&folded);
                    }
                    self.kind[i].set(Kind::Free);
                    self.free.push(i as u32);
                }
                Kind::Free => self.free.push(i as u32),
                Kind::Permanent => {}
            }
        }
        self.temp_new = 0;
    }

    /// The text of every live atom, in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Every live atom's text, in atom order.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.text
            .iter()
            .zip(&self.kind)
            .filter(|(_, k)| k.get() != Kind::Free)
            .map(|(t, _)| t.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_text_twice_yields_the_same_atom() {
        let mut i = Interner::default();
        let a = i.intern_exact("allies");
        let b = i.intern_exact("allies");
        let c = i.intern_exact("axis");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "allies");
    }

    /// Retail folds identifiers and field names but not string values:
    /// `level.myField` and `level.myfield` are one field, while `"ABC"` and
    /// `"abc"` are two distinct strings
    /// (tests/fixtures/semantics/retail-captures.txt).
    #[test]
    fn folding_is_per_role_not_global() {
        let mut i = Interner::default();
        assert_eq!(i.intern_folded("myField"), i.intern_folded("myfield"));
        assert_ne!(i.intern_exact("ABC"), i.intern_exact("abc"));
        assert_eq!(i.intern_exact("abc"), i.intern_exact("abc"));
        // A case-preserving intern still returns exactly what went in.
        let a = i.intern_exact("MedFire");
        assert_eq!(i.resolve(a), "MedFire");
    }

    /// gsc identifiers, paths and event names are matched case-insensitively
    /// by the engine, so two spellings of one key still intern to the same
    /// atom in those roles. `resolve` does not fold, though: it returns
    /// whichever spelling was interned first, verbatim.
    #[test]
    fn interning_folded_keeps_the_first_spelling() {
        let mut i = Interner::default();
        assert_eq!(
            i.intern_folded("PlayerConnect"),
            i.intern_folded("playerconnect")
        );
        let main = i.intern_folded("MaIn");
        assert_eq!(i.resolve(main), "MaIn");
    }

    #[test]
    fn resolve_folded_returns_the_lowercase_form_regardless_of_spelling() {
        let mut i = Interner::default();
        let a = i.intern_exact("MaIn");
        assert_eq!(i.resolve_folded(a), "main");
    }

    /// The bridge the notify machinery runs on: an event name that reached
    /// the VM as a case-preserving string value still folds onto the atom
    /// every other spelling of it shares.
    #[test]
    fn fold_atom_maps_every_spelling_onto_one_atom() {
        let mut i = Interner::default();
        let explode = i.intern_exact("Explode");
        let lower = i.intern_exact("explode");
        assert_ne!(explode, lower);
        assert_eq!(i.fold_atom(explode), i.fold_atom(lower));
        assert_eq!(i.fold_atom(lower), i.intern_folded("EXPLODE"));
        // Folding is idempotent, so a doubly-folded atom is still the same.
        let once = i.fold_atom(explode);
        assert_eq!(i.fold_atom(once), once);
    }

    /// Whichever role interns a spelling first owns the folded key, so the
    /// two interns can never hand out different atoms for one identifier.
    #[test]
    fn an_exact_intern_claims_the_folded_key_it_is_the_first_spelling_of() {
        let mut i = Interner::default();
        let literal = i.intern_exact("Panzerfaust");
        assert_eq!(i.intern_folded("panzerfaust"), literal);
        assert_eq!(i.resolve(literal), "Panzerfaust");
    }

    /// The mirror: a folded intern claims its own exact spelling, so a later
    /// string literal with that exact text is the same atom rather than a
    /// second atom holding identical text (which would make `==` on two
    /// equal strings false).
    #[test]
    fn a_folded_intern_claims_its_own_exact_spelling() {
        let mut i = Interner::default();
        let field = i.intern_folded("medFire");
        assert_eq!(i.intern_exact("medFire"), field);
        assert_ne!(i.intern_exact("medfire"), field);
    }

    /// A sweep frees the temporary atoms nothing marked and their keys, and
    /// keeps a pinned one, a marked one and the temporary atom owning a
    /// marked one's folded key; a freed atom goes to the next intern.
    #[test]
    fn a_sweep_frees_only_unmarked_temporary_atoms() {
        let mut i = Interner::default();
        let event = i.intern_temp("Explode");
        let garbage = i.intern_temp("garbage");
        let owner = i.intern_temp("Ev");
        let spelling = i.intern_temp("ev");
        assert_eq!(i.fold_atom(spelling), owner);
        // The host interning any spelling pins the atom owning the key.
        assert_eq!(i.intern_folded("EXPLODE"), event);
        let mut marks = i.marks();
        marks[spelling.0 as usize] = true;
        i.sweep(marks);
        assert_eq!(i.resolve(event), "Explode");
        assert_eq!(i.resolve(spelling), "ev");
        assert_eq!(i.fold_atom(spelling), owner);
        assert_eq!(i.resolve(owner), "Ev");
        assert_eq!(i.lookup_exact("garbage"), None);
        assert_eq!(i.texts().count(), 3);
        assert_eq!(i.bytes(), "Explode".len() + "Ev".len() + "ev".len());
        assert_eq!(i.intern_exact("new"), garbage);
        assert_eq!(i.resolve(garbage), "new");
        // Unmarked now, the two temporary ones go.
        i.sweep(i.marks());
        assert_eq!(i.lookup_folded("ev"), None);
        assert_eq!(i.intern_folded("explode"), event);
    }

    #[test]
    fn get_exact_does_not_intern() {
        let mut i = Interner::default();
        assert!(i.get_exact("nope").is_none());
        let a = i.intern_exact("yes");
        assert_eq!(i.get_exact("yes"), Some(a));
        assert_eq!(i.get_exact("YES"), None);
        assert_eq!(i.get_folded("YES"), Some(a));
    }
}
