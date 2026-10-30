//! What a material's image name says about the image
//! (docs/research/cod2-materials.md 10): `$name` is a code image,
//! `~src-spec&src-spec` one the converter packed from sources, anything
//! else `images/<name>.iwi`. The packing reading is INFERRED from the names.

/// A material image reference, classified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageName<'a> {
    /// `$white`, `$identitynormalmap`, ...: the name without the `$`.
    Code(&'a str),
    Packed(Packed<'a>),
    File(&'a str),
}

/// `~<src>-<spec>(&<src>-<spec>)*`, optionally cut short and ended with
/// `~<8 hex>` past about 40 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packed<'a> {
    pub parts: Vec<PackedPart<'a>>,
    /// The hash ending a name that was cut short; its last part is then
    /// incomplete.
    pub hash: Option<u32>,
}

/// One source image and the channels it fills, in output order: `gggr`
/// fills rgb from its green and alpha from its red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedPart<'a> {
    pub source: &'a str,
    /// The channel spec after the first `-`; empty when cut short.
    pub spec: &'a str,
}

/// A channel letter of a spec (`r`, `g`, `b`, or `l` the luminance) and its
/// number (`r-25`, `l08`), whose meaning is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    pub source: char,
    pub modifier: Option<i32>,
}

pub fn parse(name: &str) -> ImageName<'_> {
    if let Some(code) = name.strip_prefix('$') {
        return ImageName::Code(code);
    }
    let Some(body) = name.strip_prefix('~') else {
        return ImageName::File(name);
    };
    let (body, hash) = match body.rsplit_once('~') {
        Some((head, tail)) if tail.len() == 8 => match u32::from_str_radix(tail, 16) {
            Ok(h) => (head, Some(h)),
            Err(_) => (body, None),
        },
        _ => (body, None),
    };
    let parts = body
        .split('&')
        .map(|p| {
            let (source, spec) = p.split_once('-').unwrap_or((p, ""));
            PackedPart { source, spec }
        })
        .collect();
    ImageName::Packed(Packed { parts, hash })
}

impl PackedPart<'_> {
    /// The spec's channels; `None` when it is not letters `rgbl` each with
    /// an optional signed number.
    pub fn channels(&self) -> Option<Vec<Channel>> {
        let b = self.spec.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let source = b[i] as char;
            if !matches!(source, 'r' | 'g' | 'b' | 'l') {
                return None;
            }
            i += 1;
            let start = i;
            if b.get(i) == Some(&b'-') {
                i += 1;
            }
            let digits = i;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            let modifier = if i > digits {
                Some(self.spec[start..i].parse().ok()?)
            } else if i > start {
                return None;
            } else {
                None
            };
            out.push(Channel { source, modifier });
        }
        (!out.is_empty()).then_some(out)
    }
}

impl<'a> Packed<'a> {
    /// The source of a `~<src>-gggr` normal map: rgb its green, alpha its
    /// red, the `.wy` the shaders read as the normal's (y, x).
    pub fn normal_map_source(&self) -> Option<&'a str> {
        match self.parts[..] {
            [
                PackedPart {
                    source,
                    spec: "gggr",
                },
            ] if self.hash.is_none() => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(name: &str) -> Packed<'_> {
        match parse(name) {
            ImageName::Packed(p) => p,
            other => panic!("{name}: {other:?}"),
        }
    }

    #[test]
    fn classifies_names() {
        assert_eq!(parse("$white"), ImageName::Code("white"));
        assert_eq!(parse("caen_wall_01"), ImageName::File("caen_wall_01"));
        let n = packed("~mattress_n-gggr");
        assert_eq!(n.normal_map_source(), Some("mattress_n"));
        assert_eq!(
            n.parts[0].channels().unwrap(),
            [
                Channel {
                    source: 'g',
                    modifier: None
                },
                Channel {
                    source: 'g',
                    modifier: None
                },
                Channel {
                    source: 'g',
                    modifier: None
                },
                Channel {
                    source: 'r',
                    modifier: None
                },
            ]
        );
    }

    #[test]
    fn spec_maps() {
        let s = packed("~weapon_g43_s-r-59g-59b-59&weapon_g43_cp-l-11");
        assert_eq!(s.normal_map_source(), None);
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[1].source, "weapon_g43_cp");
        let c = s.parts[0].channels().unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(
            c[2],
            Channel {
                source: 'b',
                modifier: Some(-59)
            }
        );
        let l = s.parts[1].channels().unwrap();
        assert_eq!(
            l,
            [Channel {
                source: 'l',
                modifier: Some(-11)
            }]
        );

        let w = packed("~$white-rgb&caen_mud_cosine_02-l");
        assert_eq!(w.parts[0].source, "$white");
        let one = packed("~v_gutter01_s-rgbl08");
        assert_eq!(one.parts[0].channels().unwrap()[3].modifier, Some(8));
        assert_eq!(
            PackedPart {
                source: "x",
                spec: "rq"
            }
            .channels(),
            None
        );
        assert_eq!(
            PackedPart {
                source: "x",
                spec: "r-"
            }
            .channels(),
            None
        );
    }

    #[test]
    fn cut_short_names_keep_their_hash() {
        let t = packed("~stalingrad_brickwall02_s-rgb&stalingrad_br~5d8ed95b");
        assert_eq!(t.hash, Some(0x5d8e_d95b));
        assert_eq!(
            t.parts[1],
            PackedPart {
                source: "stalingrad_br",
                spec: ""
            }
        );
        let t = packed("~a-rgb&$~ae6da276");
        assert_eq!(t.parts[1].source, "$");
        let t = packed("~x_n-gggr~0f43101e");
        assert_eq!(t.normal_map_source(), None);
    }
}
