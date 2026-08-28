//! Interned script strings. The engine interns every script string
//! (`GScr_AllocString`), which is why `Atom` equality is a `u32` compare.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Atom(pub u32);

#[derive(Default)]
pub struct Interner {
    by_text: HashMap<String, Atom>,
    text: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> Atom {
        let folded = s.to_ascii_lowercase();
        if let Some(&a) = self.by_text.get(&folded) {
            return a;
        }
        let a = Atom(self.text.len() as u32);
        self.text.push(folded.clone());
        self.by_text.insert(folded, a);
        a
    }

    pub fn get(&self, s: &str) -> Option<Atom> {
        self.by_text.get(&s.to_ascii_lowercase()).copied()
    }

    pub fn resolve(&self, a: Atom) -> &str {
        &self.text[a.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_text_twice_yields_the_same_atom() {
        let mut i = Interner::default();
        let a = i.intern("allies");
        let b = i.intern("allies");
        let c = i.intern("axis");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "allies");
    }

    /// gsc identifiers and script paths are matched case-insensitively by
    /// the engine, so the interner folds case and `resolve` returns the
    /// folded form.
    #[test]
    fn interning_folds_case() {
        let mut i = Interner::default();
        assert_eq!(i.intern("PlayerConnect"), i.intern("playerconnect"));
        let main = i.intern("MaIn");
        assert_eq!(i.resolve(main), "main");
    }

    #[test]
    fn get_does_not_intern() {
        let mut i = Interner::default();
        assert!(i.get("nope").is_none());
        let a = i.intern("yes");
        assert_eq!(i.get("YES"), Some(a));
    }
}
