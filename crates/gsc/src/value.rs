//! Script values. gsc is dynamically typed; every slot holds one of these.

use crate::atom::{Atom, Interner};
use crate::vm::ErrorKind;

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
    /// Retail gives only numbers a boolean reading; a string, vector or
    /// `undefined` in a condition is a runtime error there
    /// (tests/fixtures/semantics/retail-captures.txt). That is why the
    /// corpus spells every such test as `isDefined(x)` or `x != ""`.
    pub fn as_bool(&self) -> Result<bool, ErrorKind> {
        match self {
            Value::Int(i) => Ok(*i != 0),
            Value::Float(f) => Ok(*f != 0.0),
            Value::Undefined => Err(ErrorKind::BadType("cannot cast undefined to bool")),
            Value::String(_) | Value::Localized(_) => {
                Err(ErrorKind::BadType("cannot cast a string to bool"))
            }
            Value::Vector(_) => Err(ErrorKind::BadType("cannot cast a vector to bool")),
            _ => Err(ErrorKind::BadType("cannot cast that to bool")),
        }
    }
}

/// How a value reads when concatenated onto a string or compared against
/// one. Measured against retail in
/// tests/fixtures/semantics/retail-captures.txt (`# probe_concat`,
/// `# probe_concat_vec`). `None` for a value retail refuses to render,
/// which the caller turns into an error.
pub fn format_number(v: Value, interner: &Interner) -> Option<String> {
    match v {
        Value::Int(i) => Some(i.to_string()),
        Value::Float(f) => Some(format_g(f)),
        Value::String(a) | Value::Localized(a) => Some(interner.resolve(a).to_string()),
        Value::Vector(v) => Some(format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2])),
        _ => None,
    }
}

/// C's `%g` with the default precision of 6. No probe reached the exponent
/// form, so it is spelled Rust's way (`1e6`) rather than C's (`1e+06`).
fn format_g(f: f32) -> String {
    if !f.is_finite() {
        return format!("{f}");
    }
    if f == 0.0 {
        return "0".to_string();
    }
    let exp = f.abs().log10().floor() as i32;
    let mut s = if !(-4..6).contains(&exp) {
        format!("{f:e}")
    } else {
        format!("{:.*}", (5 - exp).max(0) as usize, f)
    };
    if s.contains('.') && !s.contains('e') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Retail gives a boolean reading to numbers and nothing else; every
    /// other type is a fatal `cannot cast X to bool`
    /// (tests/fixtures/semantics/retail-captures.txt, the `# probe_truthy*`
    /// groups).
    #[test]
    fn only_numbers_have_a_boolean_reading() {
        assert_eq!(Value::Int(0).as_bool(), Ok(false));
        assert_eq!(Value::Int(1).as_bool(), Ok(true));
        assert_eq!(Value::Int(-1).as_bool(), Ok(true));
        assert_eq!(Value::Float(0.0).as_bool(), Ok(false));
        // IEEE `-0.0 == 0.0`, so negative zero is false too.
        assert_eq!(Value::Float(-0.0).as_bool(), Ok(false));
        assert_eq!(Value::Float(0.5).as_bool(), Ok(true));
        // NAN compares unequal to everything, including 0.0, so it reads true.
        assert_eq!(Value::Float(f32::NAN).as_bool(), Ok(true));

        let mut i = Interner::default();
        assert!(Value::String(i.intern_exact("")).as_bool().is_err());
        assert!(Value::String(i.intern_exact("a")).as_bool().is_err());
        assert!(Value::Undefined.as_bool().is_err());
        assert!(Value::Vector([0.0, 0.0, 0.0]).as_bool().is_err());
        assert!(Value::Vector([1.0, 0.0, 0.0]).as_bool().is_err());
        assert!(Value::Entity(EntId(0)).as_bool().is_err());
    }

    /// Every value in tests/fixtures/semantics/retail-captures.txt's
    /// `# probe_concat` and `# probe_concat_vec` groups.
    #[test]
    fn numbers_render_the_way_retail_renders_them() {
        let i = Interner::default();
        let g = |v| format_number(v, &i).unwrap();
        assert_eq!(g(Value::Int(5)), "5");
        assert_eq!(g(Value::Int(-5)), "-5");
        assert_eq!(g(Value::Int(1000000)), "1000000");
        assert_eq!(g(Value::Float(0.5)), "0.5");
        assert_eq!(g(Value::Float(2.0)), "2");
        assert_eq!(g(Value::Float(0.8)), "0.8");
        assert_eq!(g(Value::Float(1.0 / 3.0)), "0.333333");
        assert_eq!(g(Value::Vector([1.0, 2.0, 3.0])), "(1.00, 2.00, 3.00)");
        assert_eq!(format_number(Value::Undefined, &i), None);
    }

    /// The edges either side of the measured values: six significant
    /// digits, a dropped trailing point, and the sign carried through.
    #[test]
    fn format_g_rounds_to_six_significant_digits() {
        let i = Interner::default();
        let g = |v| format_number(v, &i).unwrap();
        assert_eq!(g(Value::Float(0.0)), "0");
        assert_eq!(g(Value::Float(-0.5)), "-0.5");
        assert_eq!(g(Value::Float(123456.7)), "123457");
        assert_eq!(g(Value::Float(1.5)), "1.5");
    }
}
