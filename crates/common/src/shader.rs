//! Q3-style shader scripts: the type model and the block splitter shared by
//! every later parsing pass.

#[derive(Debug, Clone, PartialEq)]
pub enum BlendFactor {
    Zero,
    One,
    DstColor,
    OneMinusDstColor,
    OneMinusSrcColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WaveForm {
    Sin,
    Square,
    Triangle,
    Sawtooth,
    InverseSawtooth,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wave {
    pub form: WaveForm,
    pub base: f32,
    pub amp: f32,
    pub phase: f32,
    pub freq: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TcMod {
    Scroll(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
    Stretch(Wave),
    Turb { amp: f32, phase: f32, freq: f32 },
    Transform([f32; 6]),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImageRef {
    Path(String),
    Lightmap,
    White,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnimSpec {
    pub fps: f32,
    pub paths: Vec<String>,
}

/// One `map`/`animMap` group inside a stage; a stage has one or more.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub image: ImageRef,
    pub anim: Option<AnimSpec>,
    pub clamp: bool,
    pub tcmods: Vec<TcMod>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaFunc {
    Gt0,
    Lt128,
    Ge128,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RgbGen {
    IdentityLighting,
    Identity,
    ExactVertex,
    Vertex,
    Const([f32; 3]),
    ConstLighting([f32; 3]),
    Wave(Wave),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaGen {
    Identity,
    Vertex,
    Const(f32),
    Wave(Wave),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub bundles: Vec<Bundle>,
    pub blend: Option<(BlendFactor, BlendFactor)>,
    /// `depthWrite` seen explicitly; absent means stage-default.
    pub depth_write: Option<bool>,
    pub alpha_func: Option<AlphaFunc>,
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyParms {
    pub env: String,
    pub cloud_height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceBits {
    pub sky: bool,
    pub nodraw: bool,
    pub trans: bool,
    pub water: bool,
    pub nonsolid: bool,
    pub nolightmap: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shader {
    pub name: String,
    pub stages: Vec<Stage>,
    pub two_sided: bool,
    pub sort: Option<f32>,
    pub polygon_offset: bool,
    pub nopicmip: bool,
    pub nomipmaps: bool,
    pub sky: Option<SkyParms>,
    pub sunfile: Option<String>,
    pub surface: SurfaceBits,
}

/// Script tokens: comments (`//` to end of line, `/* */` across lines) gone,
/// glued braces split off as standalone `{`/`}` tokens. Slices point into
/// `text`; delimiters are ASCII, so token boundaries are always char edges.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    std::iter::from_fn(move || loop {
        match bytes.get(i) {
            None => return None,
            Some(b'/') if bytes.get(i + 1) == Some(&b'/') => {
                i = bytes[i + 2..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(bytes.len(), |p| i + 2 + p);
            }
            Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |p| i + 2 + p + 2);
            }
            Some(c) if c.is_ascii_whitespace() => i += 1,
            Some(b'{') | Some(b'}') => {
                let tok = &text[i..i + 1];
                i += 1;
                return Some(tok);
            }
            Some(_) => {
                let start = i;
                while let Some(&c) = bytes.get(i) {
                    // a comment opener ends the word even without whitespace
                    if c.is_ascii_whitespace() || c == b'{' || c == b'}' {
                        break;
                    }
                    if c == b'/' && matches!(bytes.get(i + 1), Some(b'/' | b'*')) {
                        break;
                    }
                    i += 1;
                }
                return Some(&text[start..i]);
            }
        }
    })
}

/// Split a shader script into `(name, body)` pairs in file order.
///
/// Names are lowercased with `\` normalized to `/`; bodies are verbatim token
/// slices of `text` with comments gone and braces flattened into the stream as
/// standalone `{`/`}` tokens (same approach as `assets.rs`'s brace handling).
/// Anonymous top-level blocks come out with an empty name; duplicate names are
/// both returned (last-wins dedupe happens at map level). An unterminated
/// block at EOF is dropped.
pub fn split_blocks(text: &str) -> Vec<(String, Vec<&str>)> {
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut pending: Option<String> = None;
    let mut name: Option<String> = None;
    let mut body: Vec<&str> = Vec::new();
    for tok in tokens(text) {
        match tok {
            "{" => {
                depth += 1;
                if depth == 1 {
                    name = pending.take();
                } else {
                    body.push(tok);
                }
            }
            "}" => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        out.push((name.take().unwrap_or_default(), std::mem::take(&mut body)));
                    } else {
                        body.push(tok);
                    }
                }
            }
            _ if depth == 0 => pending = Some(tok.to_lowercase().replace('\\', "/")),
            _ => body.push(tok),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
textures\walls\big // the big wall
{
    map textures/walls/big.tga
    blendFunc GL_ONE GL_ONE
}{
    map $lightmap
}
/*
textures/walls/gone
{
    pb_never
}
*/
common\detail
{
    nopicmip
    map textures\detail\detail.tga
}
"#;

    #[test]
    fn splits_blocks_with_comments_and_glued_braces() {
        let blocks = split_blocks(SAMPLE);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0],
            (
                "textures/walls/big".to_string(),
                vec![
                    "map",
                    "textures/walls/big.tga",
                    "blendFunc",
                    "GL_ONE",
                    "GL_ONE"
                ]
            )
        );
        // glued `}{`: anonymous block keeps its body
        assert_eq!(blocks[1].0, "");
        assert_eq!(blocks[1].1, vec!["map", "$lightmap"]);
        // block-commented shader never appears
        assert!(blocks.iter().all(|(n, _)| n != "textures/walls/gone"));
        assert!(!blocks.iter().any(|(_, b)| b.contains(&"pb_never")));
    }

    #[test]
    fn normalizes_names_to_lowercase_and_slashes() {
        let blocks = split_blocks(SAMPLE);
        assert_eq!(blocks[2].0, "common/detail");
        assert_eq!(
            blocks[2].1,
            vec!["nopicmip", "map", r"textures\detail\detail.tga"]
        );
    }

    #[test]
    fn duplicate_names_both_come_out() {
        let blocks = split_blocks("a { one }\na\n{\n two \n}");
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|(n, _)| n == "a"));
        assert_eq!(blocks[0].1, vec!["one"]);
        assert_eq!(blocks[1].1, vec!["two"]);
    }

    #[test]
    fn nested_braces_flatten_into_body() {
        let blocks = split_blocks("s { { map x.tga } }");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "s");
        assert_eq!(blocks[0].1, vec!["{", "map", "x.tga", "}"]);
    }

    #[test]
    fn unterminated_block_dropped() {
        assert!(split_blocks("x { never closed").is_empty());
    }

    #[test]
    fn stray_closer_ignored() {
        let blocks = split_blocks("} x { y }");
        assert_eq!(blocks, vec![("x".to_string(), vec!["y"])]);
    }
}
