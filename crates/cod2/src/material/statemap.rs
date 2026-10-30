//! `materials/statemaps/<name>.sm`: per pass rewrites of the material's
//! stateBits (docs/research/cod2-materials.md 3, 6), compiled the way
//! `Material_ParseRuleSet` compiles them.

use super::StateBits;
use super::lex::{Kind, Lexer};
use anyhow::{Result, bail};

/// The eleven rule sets, in the order the loader demands them.
pub const GROUP_NAMES: [&str; 11] = [
    "alphaTest",
    "blendFunc",
    "separateAlphaBlendFunc",
    "cullFace",
    "depthTest",
    "depthWrite",
    "colorWrite",
    "fog",
    "polygonOffset",
    "stencil",
    "wireframe",
];

/// At most 256 rules a group.
const MAX_RULES: usize = 256;

/// `MaterialStateMapRule`: a pass applies the first rule whose
/// `material bits & mask == value`, as `bits = bits & keep | set`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StateRule {
    pub mask: StateBits,
    pub value: StateBits,
    pub set: StateBits,
    /// The complement of the bits the body writes (`stateBitsClear`
    /// inverted once the group is parsed); all ones for `passthrough`.
    pub keep: StateBits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateMap {
    pub groups: [Vec<StateRule>; 11],
}

type Values = &'static [(&'static str, u32)];

const BLEND: Values = &[
    ("Zero", 1),
    ("One", 2),
    ("SrcColor", 3),
    ("InvSrcColor", 4),
    ("SrcAlpha", 5),
    ("InvSrcAlpha", 6),
    ("DestAlpha", 7),
    ("InvDestAlpha", 8),
    ("DestColor", 9),
    ("InvDestColor", 10),
    ("BlendFactor", 12),
    ("InvBlendFactor", 13),
];
const BLEND_OP: Values = &[
    ("Disable", 0),
    ("Add", 1),
    ("Subtract", 2),
    ("RevSubtract", 3),
    ("Min", 4),
    ("Max", 5),
];
const ALPHA_TEST: Values = &[
    ("Always", 0x800),
    ("GE128", 0x3000),
    ("GT0", 0x1000),
    ("LT128", 0x2000),
];
const CULL: Values = &[("None", 0x4000), ("Back", 0x8000), ("Front", 0xc000)];
const DEPTH_TEST: Values = &[
    ("Disable", 2),
    ("LessEqual", 4),
    ("Equal", 8),
    ("Always", 0xc),
];
const POLYGON_OFFSET: Values = &[("0", 0), ("1", 0x10), ("2", 0x20)];
const STENCIL: Values = &[("Disable", 0), ("OneSided", 0x40), ("TwoSided", 0xc0)];
const STENCIL_FUNC: Values = &[
    ("Never", 0),
    ("Less", 1),
    ("Equal", 2),
    ("LessEqual", 3),
    ("Greater", 4),
    ("NotEqual", 5),
    ("GreaterEqual", 6),
    ("Always", 7),
];
const STENCIL_OP: Values = &[
    ("Keep", 0),
    ("Zero", 1),
    ("Replace", 2),
    ("IncrSat", 3),
    ("DecrSat", 4),
    ("Invert", 5),
    ("Incr", 6),
    ("Decr", 7),
];

/// A field of one table row: its names (values shifted by `shift`) and its
/// mask words. A row's word is 0 when its word 0 mask is set, else 1.
#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    values: Values,
    shift: u32,
    mask: StateBits,
}

const fn f(name: &'static str, values: Values, shift: u32, mask: StateBits) -> Field {
    Field {
        name,
        values,
        shift,
        mask,
    }
}

impl Field {
    fn word(&self) -> usize {
        if self.mask[0] != 0 { 0 } else { 1 }
    }

    fn value(&self, name: &str) -> Option<u32> {
        self.values
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, v)| v << self.shift)
    }
}

const ENABLE_RGB: Values = &[("Enable", 0x0800_0000), ("Disable", 0)];
const ENABLE_ALPHA: Values = &[("Enable", 0x1000_0000), ("Disable", 0)];
const ENABLE_FOG: Values = &[("Enable", 0x2000_0000), ("Disable", 0)];
const ENABLE_DEPTH_WRITE: Values = &[("Enable", 1), ("Disable", 0)];
const ENABLE_WIREFRAME: Values = &[("Enable", 0x8000_0000), ("Disable", 0)];

/// `_s_stateMapSrcBitGroup`: what a condition may test. The back-face
/// stencil rows carry the front masks with values 16 bits up, as retail's
/// table does, so a test of one never matches.
const SOURCE: &[Field] = &[
    f("mtlAlphaTest", ALPHA_TEST, 0, [0x3800, 0]),
    f("mtlBlendOp", BLEND_OP, 8, [0x700, 0]),
    f("mtlSrcBlend", BLEND, 0, [0xf, 0]),
    f("mtlDestBlend", BLEND, 4, [0xf0, 0]),
    f("mtlBlendOpAlpha", BLEND_OP, 24, [0x0700_0000, 0]),
    f("mtlSrcBlendAlpha", BLEND, 16, [0xf_0000, 0]),
    f("mtlDestBlendAlpha", BLEND, 20, [0xf0_0000, 0]),
    f("mtlCullFace", CULL, 0, [0xc000, 0]),
    f("mtlColorWriteRgb", ENABLE_RGB, 0, [0x0800_0000, 0]),
    f("mtlColorWriteAlpha", ENABLE_ALPHA, 0, [0x1000_0000, 0]),
    f("mtlFog", ENABLE_FOG, 0, [0x2000_0000, 0]),
    f("mtlDepthTest", DEPTH_TEST, 0, [0, 0xe]),
    f("mtlDepthWrite", ENABLE_DEPTH_WRITE, 0, [0, 1]),
    f("mtlPolygonOffset", POLYGON_OFFSET, 0, [0, 0x30]),
    f("mtlStencil", STENCIL, 0, [0, 0xc0]),
    f("mtlStencilFuncFront", STENCIL_FUNC, 17, [0, 0xe_0000]),
    f("mtlStencilOpFrontPass", STENCIL_OP, 8, [0, 0x700]),
    f("mtlStencilOpFrontFail", STENCIL_OP, 11, [0, 0x3800]),
    f("mtlStencilOpFrontZFail", STENCIL_OP, 14, [0, 0x1_c000]),
    f("mtlStencilFuncBack", STENCIL_FUNC, 29, [0, 0xe_0000]),
    f("mtlStencilOpBackPass", STENCIL_OP, 20, [0, 0x700]),
    f("mtlStencilOpBackFail", STENCIL_OP, 23, [0, 0x3800]),
    f("mtlStencilOpBackZFail", STENCIL_OP, 26, [0, 0x1_c000]),
];

/// The destination tables (0x0036b140..0x0036ba00): a body's comma
/// separated values, in this order per group. The masks are retail's,
/// quirks included: the rgb source factor clears 0x70f and the back
/// stencil fields clear the front masks.
const DEST: [&[Field]; 11] = [
    &[f("", ALPHA_TEST, 0, [0x3800, 0])],
    &[
        f("", BLEND_OP, 8, [0x700, 0]),
        f("", BLEND, 0, [0x70f, 0]),
        f("", BLEND, 4, [0x7f0, 0]),
    ],
    &[
        f("", BLEND_OP, 24, [0x0700_0000, 0]),
        f("", BLEND, 16, [0xf_0000, 0]),
        f("", BLEND, 20, [0xf0_0000, 0]),
    ],
    &[f("", CULL, 0, [0xc000, 0])],
    &[f("", DEPTH_TEST, 0, [0, 0xe])],
    &[f("", ENABLE_DEPTH_WRITE, 0, [0, 1])],
    &[
        f("", ENABLE_RGB, 0, [0x0800_0000, 0]),
        f("", ENABLE_ALPHA, 0, [0x1000_0000, 0]),
    ],
    &[f("", ENABLE_FOG, 0, [0x2000_0000, 0])],
    &[f("", POLYGON_OFFSET, 0, [0, 0x30])],
    &[
        f("", STENCIL, 0, [0, 0xc0]),
        f("", STENCIL_FUNC, 17, [0, 0xe_0000]),
        f("", STENCIL_OP, 8, [0, 0x700]),
        f("", STENCIL_OP, 11, [0, 0x3800]),
        f("", STENCIL_OP, 14, [0, 0x1_c000]),
        f("", STENCIL_FUNC, 29, [0, 0xe_0000]),
        f("", STENCIL_OP, 20, [0, 0x700]),
        f("", STENCIL_OP, 23, [0, 0x3800]),
        f("", STENCIL_OP, 26, [0, 0x1_c000]),
    ],
    &[f("", ENABLE_WIREFRAME, 0, [0x8000_0000, 0])],
];

fn token_text<'a>(lx: &mut Lexer<'a>) -> Result<(&'a str, u32)> {
    let t = lx.next()?;
    if t.kind == Kind::Quoted {
        bail!("line {}: unexpected quoted '{}'", t.line, t.text);
    }
    Ok((t.text, t.line))
}

/// One `name == value` test ORed into `rule`.
fn condition(lx: &mut Lexer, field: &Field, rule: &mut StateRule) -> Result<()> {
    lx.expect("==")?;
    let (v, line) = token_text(lx)?;
    let Some(value) = field.value(v) else {
        bail!("line {line}: {v} is not a valid state value");
    };
    let w = field.word();
    rule.mask[w] |= field.mask[w];
    rule.value[w] |= value;
    Ok(())
}

fn rule_set(lx: &mut Lexer, group: usize) -> Result<Vec<StateRule>> {
    let name = GROUP_NAMES[group];
    lx.expect(name)?;
    lx.expect("{")?;
    let mut rules: Vec<StateRule> = Vec::new();
    // Rules from `done` on share the body still to come.
    let mut done = 0;
    loop {
        let (tok, line) = token_text(lx)?;
        if tok == "}" {
            if rules.is_empty() {
                bail!("line {line}: no entries for state {name}");
            }
            if done != rules.len() {
                bail!("line {line}: missing value for state {name}");
            }
            break;
        }
        if rules.len() == MAX_RULES {
            bail!("line {line}: state {name} has more than {MAX_RULES} rules");
        }
        if tok == "default" {
            lx.expect(":")?;
            rules.push(StateRule::default());
            continue;
        }
        if let Some(field) = SOURCE.iter().find(|s| s.name == tok) {
            let mut rule = StateRule::default();
            condition(lx, field, &mut rule)?;
            loop {
                let (sep, line) = token_text(lx)?;
                match sep {
                    ":" => break,
                    "&&" => {
                        let (next, line) = token_text(lx)?;
                        let Some(field) = SOURCE.iter().find(|s| s.name == next) else {
                            bail!("line {line}: failed parsing conditional after '&&'");
                        };
                        condition(lx, field, &mut rule)?;
                    }
                    _ => bail!("line {line}: expected ':' or '&&', found '{sep}'"),
                }
            }
            rules.push(rule);
            continue;
        }
        // Not a condition: the body of every rule since the last one.
        if done == rules.len() {
            bail!("line {line}: missing rule condition for state {name}");
        }
        let mut set = [0u32; 2];
        let mut clear = [0u32; 2];
        if tok != "passthrough" {
            let fields = DEST[group];
            let mut v = tok;
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    lx.expect(",")?;
                    v = token_text(lx)?.0;
                }
                let Some(value) = field.value(v) else {
                    bail!("line {line}: {v} is not a valid state value");
                };
                let w = field.word();
                set[w] |= value;
                clear[w] |= field.mask[w];
            }
        }
        lx.expect(";")?;
        for r in &mut rules[done..] {
            r.set = set;
            r.keep = [!clear[0], !clear[1]];
        }
        done = rules.len();
    }
    Ok(rules)
}

impl StateMap {
    pub fn parse(src: &str) -> Result<Self> {
        let mut lx = Lexer::new(src)?;
        let mut groups: [Vec<StateRule>; 11] = Default::default();
        for (i, g) in groups.iter_mut().enumerate() {
            *g = rule_set(&mut lx, i)?;
        }
        if let Some(t) = lx.peek() {
            bail!("line {}: '{}' after the last rule set", t.line, t.text);
        }
        Ok(Self { groups })
    }

    /// A pass's state, as `RB_DrawSingleTechnique` builds it: each group's
    /// first rule matching the material's own bits rewrites the running
    /// copy. `Err` names the group no rule matched.
    pub fn apply(&self, material: StateBits) -> Result<StateBits, usize> {
        let mut bits = material;
        for (g, rules) in self.groups.iter().enumerate() {
            let Some(r) = rules.iter().find(|r| {
                material[0] & r.mask[0] == r.value[0] && material[1] & r.mask[1] == r.value[1]
            }) else {
                return Err(g);
            };
            for ((b, keep), set) in bits.iter_mut().zip(r.keep).zip(r.set) {
                *b = *b & keep | set;
            }
        }
        Ok(bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock `fogged.sm`'s rules, rewritten.
    const FOGGED: &str = "
alphaTest {
    mtlAlphaTest == Always && mtlBlendOp == Add && mtlSrcBlend == SrcAlpha && mtlDestBlend == InvSrcAlpha:
    mtlAlphaTest == Always && mtlBlendOp == Add && mtlSrcBlend == SrcAlpha && mtlDestBlend == One:
        GT0;
    default:
        passthrough;
}
blendFunc { default: passthrough; }
separateAlphaBlendFunc {
    mtlBlendOp == Disable:
        Disable, One, Zero;
    default:
        Add, InvDestAlpha, One;
}
cullFace { default: passthrough; }
depthTest { default: passthrough; }
depthWrite { mtlBlendOp == Disable: Enable; default: Disable; }
colorWrite { mtlPolygonOffset == 0: Enable, Enable; default: Enable, Disable; }
fog { default: passthrough; }
polygonOffset { default: passthrough; }
stencil { default: passthrough; }
wireframe { default: Disable; }
";

    #[test]
    fn compiles_rules() {
        let sm = StateMap::parse(FOGGED).unwrap();
        let at = &sm.groups[0];
        assert_eq!(at.len(), 3);
        assert_eq!(at[0].mask, [0x3fff, 0]);
        assert_eq!(at[0].value, [0x0965, 0]);
        assert_eq!(at[1].value, [0x0925, 0]);
        assert_eq!(at[0].set, [0x1000, 0]);
        assert_eq!(at[1].keep, [!0x3800, !0]);
        assert_eq!(
            at[2],
            StateRule {
                keep: [!0, !0],
                ..Default::default()
            }
        );
        let sep = &sm.groups[2][1];
        assert_eq!(sep.set, [0x0128_0000, 0]);
        assert_eq!(sep.keep, [!0x07ff_0000, !0]);
    }

    #[test]
    fn applies_against_the_material_bits() {
        let sm = StateMap::parse(FOGGED).unwrap();
        // Opaque: alpha writes come on, the rest stays.
        assert_eq!(sm.apply([0x2812_8812, 5]), Ok([0x3812_8812, 5]));
        // A decal (SrcAlpha InvSrcAlpha, polygon offset 1): GT0, separate
        // alpha (Add, InvDestAlpha, One), no depth write, no alpha write.
        assert_eq!(sm.apply([0x2812_8965, 0x14]), Ok([0x2928_9165, 0x14]));
    }

    #[test]
    fn unmatched_group_and_bad_maps() {
        let cull = FOGGED.replace(
            "cullFace { default: passthrough; }",
            "cullFace { mtlCullFace == Back: Front; mtlCullFace == Front: Back; }",
        );
        let sm = StateMap::parse(&cull).unwrap();
        assert_eq!(sm.apply([0x2812_8812, 5]), Ok([0x3812_c812, 5]));
        assert_eq!(sm.apply([0x2812_4812, 5]), Err(3));

        let bad_value = FOGGED.replace("GT0;", "GT1;");
        assert!(StateMap::parse(&bad_value).is_err());
        let order = FOGGED.replace("blendFunc {", "blendFunk {");
        assert!(StateMap::parse(&order).is_err());
        let dangling = FOGGED.replace("default: Disable; }\ncolorWrite", "default: }\ncolorWrite");
        assert!(StateMap::parse(&dangling).is_err());
        let no_cond = FOGGED.replace("fog { default: passthrough; }", "fog { passthrough; }");
        assert!(StateMap::parse(&no_cond).is_err());
        let few = FOGGED.replace("Disable, One, Zero;", "Disable, One;");
        assert!(StateMap::parse(&few).is_err());
        let extra = format!("{FOGGED} x");
        assert!(StateMap::parse(&extra).is_err());
    }
}
