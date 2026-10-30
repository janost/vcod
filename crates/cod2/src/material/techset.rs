//! `materials/techniquesets/<name>.techset`: render pass type -> technique
//! (docs/research/cod2-materials.md 4).

use super::lex::{Kind, Lexer};
use super::{TECHNIQUE_TYPE_COUNT, TechniqueType};
use anyhow::{Result, bail};

/// `MaterialTechniqueSet`: a technique name per type; `None` means the
/// material is not drawn in that pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TechniqueSet {
    pub techniques: [Option<String>; TECHNIQUE_TYPE_COUNT],
}

impl TechniqueSet {
    /// `techset := ( label+ name ';' )*`, `label := '"' typename '"' ':'`.
    pub fn parse(src: &str) -> Result<Self> {
        let mut lx = Lexer::new(src)?;
        let mut techniques: [Option<String>; TECHNIQUE_TYPE_COUNT] = std::array::from_fn(|_| None);
        let mut labels: Vec<TechniqueType> = Vec::new();
        while let Some(t) = lx.peek() {
            if t.kind == Kind::Quoted {
                lx.next()?;
                let Some(ty) = TechniqueType::from_label(t.text) else {
                    bail!("line {}: unknown technique type \"{}\"", t.line, t.text);
                };
                if labels.len() == TECHNIQUE_TYPE_COUNT {
                    bail!("line {}: too many labels in technique set", t.line);
                }
                labels.push(ty);
                lx.expect(":")?;
            } else {
                if labels.is_empty() {
                    bail!("line {}: technique '{}' has no label", t.line, t.text);
                }
                let name = lx.word()?;
                lx.expect(";")?;
                for ty in labels.drain(..) {
                    techniques[ty as usize] = Some(name.to_string());
                }
            }
        }
        if !labels.is_empty() {
            bail!("technique set ends on a label");
        }
        Ok(Self { techniques })
    }

    pub fn get(&self, ty: TechniqueType) -> Option<&str> {
        self.techniques[ty as usize].as_deref()
    }

    /// Every (type, technique) the set names, in type order.
    pub fn iter(&self) -> impl Iterator<Item = (TechniqueType, &str)> {
        TechniqueType::ALL
            .iter()
            .zip(&self.techniques)
            .filter_map(|(&ty, n)| Some((ty, n.as_deref()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_labels_and_missing_types() {
        let s = TechniqueSet::parse(
            "// c\n\"depth prepass\":\n\tzprepass;\n\n\"lightmap no fog\":\n\"lightprobe no fog\":\n\tlmap;\n",
        )
        .unwrap();
        assert_eq!(s.get(TechniqueType::DepthPrepass), Some("zprepass"));
        assert_eq!(s.get(TechniqueType::LightmapNoFog), Some("lmap"));
        assert_eq!(s.get(TechniqueType::LightprobeNoFog), Some("lmap"));
        assert_eq!(s.get(TechniqueType::Sky), None);
        assert_eq!(s.iter().count(), 3);
    }

    #[test]
    fn rejects_bad_sets() {
        assert!(TechniqueSet::parse("\"lit\":\n\tx;").is_err());
        assert!(TechniqueSet::parse("x;").is_err());
        assert!(TechniqueSet::parse("\"sky\":").is_err());
        assert!(TechniqueSet::parse("\"sky\":\n\tx").is_err());
        let many = "\"sky\":\n".repeat(35) + "x;";
        assert!(TechniqueSet::parse(&many).is_err());
    }
}
