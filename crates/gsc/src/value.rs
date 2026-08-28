//! Script values. gsc is dynamically typed; every slot holds one of these.

use crate::atom::Atom;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StructId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ArrayId(pub u32);

/// A script function, identified by its interned canonical file path and
/// name. Both halves are folded, so `MAIN` and `main` are one function.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FuncRef {
    pub file: Atom,
    pub name: Atom,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Value {
    Undefined,
    Int(i32),
    Float(f32),
    String(Atom),
    /// `&"KEY"`, resolved by the client, opaque here.
    Localized(Atom),
    Vector([f32; 3]),
    Entity(EntId),
    Struct(StructId),
    Array(ArrayId),
    Function(FuncRef),
    /// `%anim_name`. Parsed and carried; the MP scripts never read one back.
    Anim(Atom),
}

impl Value {
    /// Only `undefined` and numeric zero are false. An empty string is true,
    /// which is why `isDefined` exists at all. INFERRED from script usage,
    /// not yet pinned against the retail server.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Undefined | Value::Int(0)) && *self != Value::Float(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::Interner;

    #[test]
    fn truthiness_follows_gsc() {
        let mut i = Interner::default();
        assert!(!Value::Undefined.is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(!Value::Float(0.0).is_truthy());
        // IEEE `-0.0 == 0.0`, so negative zero is false too.
        assert!(!Value::Float(-0.0).is_truthy());
        assert!(Value::Int(1).is_truthy());
        assert!(Value::Int(-1).is_truthy());
        assert!(Value::Float(0.5).is_truthy());
        // NAN compares unequal to everything, including 0.0, so it reads true.
        assert!(Value::Float(f32::NAN).is_truthy());
        assert!(Value::String(i.intern("")).is_truthy());
        assert!(Value::Entity(EntId(0)).is_truthy());
        // Only the Float(0.0) arm is false; a zero vector isn't matched by it.
        assert!(Value::Vector([0.0, 0.0, 0.0]).is_truthy());
    }
}
