//! `materials/techniques/<name>.tech`, the dx9 grammar
//! (docs/research/cod2-materials.md 5). The shaders themselves are
//! `materials/shaders/<file>`, not read here.

use super::lex::Lexer;
use anyhow::{Result, bail};

/// At most four passes a technique (the `== 4` exit in the loader).
pub const MAX_PASSES: usize = 4;
/// At most sixteen vertex routes a pass.
pub const MAX_ROUTES: usize = 16;

/// `_s_defaultCodeSamplers`: a shader sampler the technique leaves unset is
/// bound by name to this `MaterialTextureSource` (`TEXTURE_SRC_CODE_*`).
pub const DEFAULT_CODE_SAMPLERS: [(&str, u8); 14] = [
    ("specularitySampler", 3),
    ("shadowCookieSampler", 12),
    ("feedbackSampler", 13),
    ("dynamicShadowSampler", 19),
    ("floatZSampler", 21),
    ("attenuationSampler", 17),
    ("lightmapWeightSampler", 7),
    ("lightmapSamplerR", 8),
    ("lightmapSamplerG", 9),
    ("lightmapSamplerB", 10),
    ("lightmapSamplerSun", 11),
    ("lightGridWeightSampler0", 5),
    ("lightGridWeightSampler1", 6),
    ("smodelLightingSampler", 4),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Technique {
    pub passes: Vec<Pass>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    /// `materials/statemaps/<name>.sm`.
    pub state_map: String,
    pub routes: Vec<VertexRoute>,
    pub vertex_shader: ShaderRef,
    pub pixel_shader: ShaderRef,
}

/// `vertexShader 2.0 "lmap.hlsl" { ... }`: profile `vs_2_0`, entry `vs_main`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderRef {
    pub version: (u8, u8),
    pub file: String,
    pub args: Vec<ShaderArg>,
}

/// `vertex.<dest> = code.<source>;`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexRoute {
    pub dest: StreamDest,
    pub source: StreamSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDest {
    Position,
    Normal,
    Color(u8),
    TexCoord(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamSource {
    Position,
    Normal,
    Color,
    TexCoord(u8),
    Binormal,
    Tangent,
}

impl StreamDest {
    /// The vertex declaration's destination code: position 0, normal 1,
    /// color[n] 2+n, texcoord[n] 4+n.
    pub fn code(self) -> u8 {
        match self {
            Self::Position => 0,
            Self::Normal => 1,
            Self::Color(n) => 2 + n,
            Self::TexCoord(n) => 4 + n,
        }
    }
}

impl StreamSource {
    /// The source code: position 0, normal 1, color 2, texcoord[n] 3+n,
    /// binormal 5, tangent 6.
    pub fn code(self) -> u8 {
        match self {
            Self::Position => 0,
            Self::Normal => 1,
            Self::Color => 2,
            Self::TexCoord(n) => 3 + n,
            Self::Binormal => 5,
            Self::Tangent => 6,
        }
    }
}

/// `name ( '[' row ( ',' row )? ']' )? '=' value ';'`
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderArg {
    pub name: String,
    /// A matrix's first and last row.
    pub rows: Option<(u8, u8)>,
    pub value: ArgValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArgValue {
    /// `material.<name>`: a texture or constant of the material.
    Material(String),
    /// `sampler.<name>`: a code texture (`feedback`, `outdoor`, `sky`, ...).
    Sampler(String),
    /// `constant.<name>`.
    Constant(String),
    /// `code.<path>`, its tokens joined (`filterTap[3]`).
    Code(String),
    /// `float1(...)` to `float4(...)`.
    Literal(Vec<f32>),
}

fn index(lx: &mut Lexer, max: u8) -> Result<u8> {
    lx.expect("[")?;
    let n: u8 = lx.number()?;
    lx.expect("]")?;
    if n > max {
        bail!("index {n} is past {max}");
    }
    Ok(n)
}

fn route(lx: &mut Lexer) -> Result<VertexRoute> {
    lx.expect(".")?;
    let dest = match lx.word()? {
        "position" => StreamDest::Position,
        "normal" => StreamDest::Normal,
        "color" => StreamDest::Color(index(lx, 1)?),
        "texcoord" => StreamDest::TexCoord(index(lx, 7)?),
        w => bail!("unknown vertex destination '{w}'"),
    };
    lx.expect("=")?;
    lx.expect("code")?;
    lx.expect(".")?;
    let source = match lx.word()? {
        "position" => StreamSource::Position,
        "normal" => StreamSource::Normal,
        "color" => StreamSource::Color,
        "texcoord" => StreamSource::TexCoord(index(lx, 1)?),
        "binormal" => StreamSource::Binormal,
        "tangent" => StreamSource::Tangent,
        w => bail!("unknown vertex source '{w}'"),
    };
    lx.expect(";")?;
    Ok(VertexRoute { dest, source })
}

fn arg_value(lx: &mut Lexer) -> Result<ArgValue> {
    let kind = lx.word()?;
    let count = match kind {
        "float1" => 1,
        "float2" => 2,
        "float3" => 3,
        "float4" => 4,
        _ => 0,
    };
    if count > 0 {
        lx.expect("(")?;
        let mut v = Vec::with_capacity(count);
        for i in 0..count {
            if i > 0 {
                lx.expect(",")?;
            }
            v.push(lx.number()?);
        }
        lx.expect(")")?;
        return Ok(ArgValue::Literal(v));
    }
    lx.expect(".")?;
    if kind == "code" {
        let mut path = String::new();
        while !lx.peek().is_some_and(|t| t.text == ";") {
            path.push_str(lx.next()?.text);
        }
        return Ok(ArgValue::Code(path));
    }
    let name = lx.word()?.to_string();
    Ok(match kind {
        "material" => ArgValue::Material(name),
        "sampler" => ArgValue::Sampler(name),
        "constant" => ArgValue::Constant(name),
        _ => bail!("unknown argument source '{kind}'"),
    })
}

fn shader(lx: &mut Lexer, keyword: &str) -> Result<ShaderRef> {
    lx.expect(keyword)?;
    let v = lx.word()?;
    let version = v
        .split_once('.')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .ok_or_else(|| anyhow::anyhow!("bad shader version '{v}'"))?;
    let file = lx.quoted()?.to_string();
    lx.expect("{")?;
    let mut args = Vec::new();
    while !lx.eat("}") {
        let name = lx.word()?.to_string();
        let rows = if lx.eat("[") {
            let first: u8 = lx.number()?;
            let last = if lx.eat(",") { lx.number()? } else { first };
            lx.expect("]")?;
            Some((first, last))
        } else {
            None
        };
        lx.expect("=")?;
        let value = arg_value(lx)?;
        lx.expect(";")?;
        args.push(ShaderArg { name, rows, value });
    }
    Ok(ShaderRef {
        version,
        file,
        args,
    })
}

impl Technique {
    /// `tech := pass{1,4}`.
    pub fn parse(src: &str) -> Result<Self> {
        let mut lx = Lexer::new(src)?;
        let mut passes = Vec::new();
        while !lx.at_end() {
            if passes.len() == MAX_PASSES {
                bail!("more than {MAX_PASSES} passes");
            }
            lx.expect("{")?;
            lx.expect("stateMap")?;
            let state_map = lx.quoted()?.to_string();
            lx.expect(";")?;
            let mut routes = Vec::new();
            while lx.eat("vertex") {
                if routes.len() == MAX_ROUTES {
                    bail!("more than {MAX_ROUTES} vertex routes");
                }
                routes.push(route(&mut lx)?);
            }
            let vertex_shader = shader(&mut lx, "vertexShader")?;
            let pixel_shader = shader(&mut lx, "pixelShader")?;
            lx.expect("}")?;
            passes.push(Pass {
                state_map,
                routes,
                vertex_shader,
                pixel_shader,
            });
        }
        if passes.is_empty() {
            bail!("technique has no pass");
        }
        Ok(Self { passes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LMAP: &str = r#"
{
	stateMap "fogged";

	vertex.position = code.position;
	vertex.color[0] = code.color;
	vertex.texcoord[0] = code.texcoord[0];
	vertex.texcoord[1] = code.texcoord[1];
	vertex.texcoord[2] = code.tangent;
	vertex.texcoord[3] = code.binormal;
	vertex.normal = code.normal;

	vertexShader 2.0 "a_efog.hlsl"
	{
	}

	pixelShader 2.0 "a.hlsl"
	{
		colorMapSampler = material.colorMap;
		outdoorMapSampler = sampler.outdoor;
		detailScale = material.detailScale;
		tint = constant.materialColor;
		taps[0, 3] = code.filterTap[2];
		bias = float4(1, -0.5, 2.25, 0);
	}
}
{
	stateMap "multiply_fog";
	vertexShader 1.1 "b.hlsl" { }
	pixelShader 1.1 "b.hlsl" { }
}
"#;

    #[test]
    fn parses_passes_routes_and_arguments() {
        let t = Technique::parse(LMAP).unwrap();
        assert_eq!(t.passes.len(), 2);
        let p = &t.passes[0];
        assert_eq!(p.state_map, "fogged");
        assert_eq!(p.routes.len(), 7);
        assert_eq!(
            p.routes[3],
            VertexRoute {
                dest: StreamDest::TexCoord(1),
                source: StreamSource::TexCoord(1)
            }
        );
        assert_eq!((p.routes[4].dest.code(), p.routes[4].source.code()), (6, 6));
        assert_eq!((p.routes[5].dest.code(), p.routes[5].source.code()), (7, 5));
        assert_eq!(p.vertex_shader.version, (2, 0));
        assert_eq!(p.vertex_shader.file, "a_efog.hlsl");
        assert!(p.vertex_shader.args.is_empty());
        let a = &p.pixel_shader.args;
        assert_eq!(a[0].value, ArgValue::Material("colorMap".into()));
        assert_eq!(a[1].value, ArgValue::Sampler("outdoor".into()));
        assert_eq!(a[3].value, ArgValue::Constant("materialColor".into()));
        assert_eq!(a[4].rows, Some((0, 3)));
        assert_eq!(a[4].value, ArgValue::Code("filterTap[2]".into()));
        assert_eq!(a[5].value, ArgValue::Literal(vec![1.0, -0.5, 2.25, 0.0]));
        assert_eq!(t.passes[1].pixel_shader.version, (1, 1));
        assert!(t.passes[1].routes.is_empty());
    }

    #[test]
    fn rejects_bad_techniques() {
        assert!(Technique::parse("").is_err());
        let no_map = "{ vertexShader 1.1 \"a\" { } pixelShader 1.1 \"a\" { } }";
        assert!(Technique::parse(no_map).is_err());
        let bad_dest = "{ stateMap \"x\"; vertex.texcoord[8] = code.position; }";
        assert!(Technique::parse(bad_dest).is_err());
        let bad_src = "{ stateMap \"x\"; vertex.normal = code.fog; }";
        assert!(Technique::parse(bad_src).is_err());
        let pass = "{ stateMap \"x\"; vertexShader 1.1 \"a\" { } pixelShader 1.1 \"a\" { } }";
        assert!(Technique::parse(&pass.repeat(4)).is_ok());
        assert!(Technique::parse(&pass.repeat(5)).is_err());
        let short = "{ stateMap \"x\"; vertexShader 1.1 \"a\" { } pixelShader 1.1 \"a\" { s = float2(1); } }";
        assert!(Technique::parse(short).is_err());
    }
}
