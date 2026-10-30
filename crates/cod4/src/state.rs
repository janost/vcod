//! A material's `GfxStateBits` (`loadBits[2]`) decoded into the blend,
//! alpha test, cull and depth state a pass draws with. Layout and evidence:
//! `docs/research/cod4-world.md` section 8.

/// `GFXS_BLEND_*`: a factor's value is its `D3DBLEND` (`s_blendTable`
/// maps 0..10 onto themselves).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Factor {
    Zero,
    One,
    SrcColor,
    InvSrcColor,
    SrcAlpha,
    InvSrcAlpha,
    DestAlpha,
    InvDestAlpha,
    DestColor,
    InvDestColor,
}

impl Factor {
    fn of(v: u32) -> Option<Self> {
        use Factor::*;
        Some(match v {
            1 => Zero,
            2 => One,
            3 => SrcColor,
            4 => InvSrcColor,
            5 => SrcAlpha,
            6 => InvSrcAlpha,
            7 => DestAlpha,
            8 => InvDestAlpha,
            9 => DestColor,
            10 => InvDestColor,
            _ => return None,
        })
    }
}

/// `GFXS_BLENDOP_*`, a `D3DBLENDOP` (`s_blendOpTable` maps 0..5 onto
/// themselves; 0 is blending off).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendOp {
    Add,
    Subtract,
    RevSubtract,
    Min,
    Max,
}

/// `GFXS0_ATEST_*`: the alpha test against the reference 0 or 128.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlphaTest {
    Gt0,
    Lt128,
    Ge128,
}

/// `GFXS0_CULL_*`, as `s_cullTable` hands it to D3D: back is
/// `D3DCULL_CCW`, front `D3DCULL_CW`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cull {
    None,
    Back,
    Front,
}

/// `GFXS1_DEPTHTEST_*`; `s_depthTestTable` turns them into `D3DCMP_ALWAYS`,
/// `LESS`, `EQUAL`, `LESSEQUAL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DepthTest {
    Disabled,
    Always,
    Less,
    Equal,
    LessEqual,
}

/// One pass's render state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GfxState {
    /// Colour source and destination factors and op; `None` with the op 0.
    pub blend: Option<(Factor, Factor, BlendOp)>,
    pub alpha_test: Option<AlphaTest>,
    pub cull: Cull,
    pub depth_write: bool,
    pub depth_test: DepthTest,
    /// `GFXS1_POLYGON_OFFSET_*`: 0 none, 1 and 2 the decal offsets, 3 the
    /// shadow map's.
    pub polygon_offset: u8,
    /// `GFXS0_COLORWRITE_RGB` and `_ALPHA`.
    pub color_write: bool,
    pub alpha_write: bool,
}

impl GfxState {
    /// Decodes `loadBits`. A blend factor outside 1..=10 reads as `One`.
    pub fn decode([w0, w1]: [u32; 2]) -> Self {
        let factor = |v: u32| Factor::of(v).unwrap_or(Factor::One);
        let op = match (w0 >> 8) & 7 {
            1 => Some(BlendOp::Add),
            2 => Some(BlendOp::Subtract),
            3 => Some(BlendOp::RevSubtract),
            4 => Some(BlendOp::Min),
            5 => Some(BlendOp::Max),
            _ => None,
        };
        let alpha_test = match w0 & 0x3800 {
            0x1000 => Some(AlphaTest::Gt0),
            0x2000 => Some(AlphaTest::Lt128),
            0x3000 => Some(AlphaTest::Ge128),
            _ => None,
        };
        let cull = match w0 & 0xC000 {
            0x8000 => Cull::Back,
            0xC000 => Cull::Front,
            _ => Cull::None,
        };
        let depth_test = if w1 & 2 != 0 {
            DepthTest::Disabled
        } else {
            match w1 & 0xC {
                0 => DepthTest::Always,
                4 => DepthTest::Less,
                8 => DepthTest::Equal,
                _ => DepthTest::LessEqual,
            }
        };
        GfxState {
            blend: op.map(|op| (factor(w0 & 0xF), factor((w0 >> 4) & 0xF), op)),
            alpha_test,
            cull,
            depth_write: w1 & 1 != 0,
            depth_test,
            polygon_offset: ((w1 >> 4) & 3) as u8,
            color_write: w0 & 0x0800_0000 != 0,
            alpha_write: w0 & 0x1000_0000 != 0,
        }
    }
}

/// `MaterialTechniqueType` slots a world or model material's state is read
/// from, in the order tried: `TECHNIQUE_LIT` (7), `TECHNIQUE_UNLIT` (4),
/// `TECHNIQUE_EMISSIVE` (5).
pub const DRAW_TECHNIQUES: [usize; 3] = [7, 4, 5];

/// `TECHNIQUE_BUILD_SHADOWMAP_COLOR`: the sun shadow map's caster pass
/// (`shadowmap_build_sm3.hlsl`, cod4-world.md 3.6).
pub const BUILD_SHADOWMAP_COLOR: usize = 3;

/// The state a material casts into the sun's shadow map with: its
/// [`BUILD_SHADOWMAP_COLOR`] entry, `None` when it has none (it casts no
/// shadow).
pub fn caster_state(entries: &[u8], bits: &[[u32; 2]]) -> Option<GfxState> {
    let e = *entries.get(BUILD_SHADOWMAP_COLOR)? as usize;
    bits.get(e).copied().map(GfxState::decode)
}

/// The state entry a material draws its lit view with: the first of
/// [`DRAW_TECHNIQUES`] whose `stateBitsEntry` names an entry.
pub fn draw_state(entries: &[u8], bits: &[[u32; 2]]) -> Option<GfxState> {
    DRAW_TECHNIQUES.iter().find_map(|&t| {
        let e = *entries.get(t)? as usize;
        bits.get(e).copied().map(GfxState::decode)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock world sets' state words (mp_crash): opaque, alpha tested,
    /// blended, multiplied decal, additive.
    #[test]
    fn decodes_the_stock_world_states() {
        let r0 = GfxState::decode([0x1812_8812, 0xD]);
        assert_eq!(r0.blend, None);
        assert_eq!(
            (r0.cull, r0.depth_write, r0.alpha_test),
            (Cull::Back, true, None)
        );
        assert_eq!(r0.depth_test, DepthTest::LessEqual);
        let t0 = GfxState::decode([0x1812_B012, 0xD]);
        assert_eq!(t0.alpha_test, Some(AlphaTest::Ge128));
        let b0 = GfxState::decode([0x1928_9162, 0xC]);
        assert_eq!(
            b0.blend,
            Some((Factor::One, Factor::InvSrcAlpha, BlendOp::Add))
        );
        assert!(!b0.depth_write);
        let mul = GfxState::decode([0x1928_8931, 0xC]);
        assert_eq!(
            mul.blend,
            Some((Factor::Zero, Factor::SrcColor, BlendOp::Add))
        );
        let add = GfxState::decode([0x1812_492A, 0xC]);
        assert_eq!(
            add.blend,
            Some((Factor::InvDestColor, Factor::One, BlendOp::Add))
        );
        assert_eq!(add.cull, Cull::None);
        assert_eq!(GfxState::decode([0x1812_8812, 0x1D]).polygon_offset, 1);
    }

    #[test]
    fn picks_the_lit_technique_first() {
        let mut entries = [255u8; 34];
        entries[4] = 0;
        entries[7] = 1;
        let bits = [[0x1812_8812, 0xD], [0x1928_9162, 0xC]];
        assert!(draw_state(&entries, &bits).unwrap().blend.is_some());
        entries[7] = 255;
        assert!(draw_state(&entries, &bits).unwrap().blend.is_none());
    }
}
