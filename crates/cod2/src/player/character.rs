//! Which models make a CoD2 player: the body and attachments a client state
//! names through the model and tag configstrings, or a stock character
//! script read off the paks when it names none.
//! docs/research/cod2-player-anims.md 1-2.

use crate::net::ClientState2;
use crate::net::delta::field_index;
use crate::net::fields::CLIENT_STATE_FIELDS;
use anyhow::{Context, Result, anyhow, bail};
use vcod_common::asset_source::AssetSource;

/// The first model configstring (`G_ModelIndex`'s base; index 0 is never
/// written). cod2-protocol.md 6.
pub const CS_MODELS: u16 = 334;
/// The first tag-name configstring (`G_TagIndex` 0x811d5cc).
pub const CS_TAGS: u16 = 110;

/// One player's models, names without the `xmodel/` prefix: the body, then
/// each attachment with the tag it hangs off (`None`: its own root bone's
/// name, the engine default for `attach(model, "")`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Character {
    pub body: String,
    pub attachments: Vec<(String, Option<String>)>,
    /// Bit `i` set: attachment `i` was attached with `ignoreCollision`, and
    /// its hit boxes take no round (`DObjTraceline`, Mac 0x76600, skips a
    /// model whose bit is set in the DObj's `+0x1a` mask). The server's
    /// alone; a client state does not carry it.
    pub ignore_collision: u32,
}

fn strip_xmodel(name: &str) -> &str {
    name.strip_prefix("xmodel/").unwrap_or(name)
}

impl Character {
    /// What a client state names: `modelindex` and the six
    /// `attachModelIndex`/`attachTagIndex` pairs through the model and tag
    /// configstrings, as `ClientEndFrame`'s client info update (0x80f72f8)
    /// wrote them. `None` while `modelindex` is 0 or names nothing.
    pub fn from_client_state<'a>(
        cs: &ClientState2,
        configstring: impl Fn(u16) -> Option<&'a str>,
    ) -> Option<Character> {
        let field = |name: &str| cs.f[field_index(&CLIENT_STATE_FIELDS, name)];
        let model = |index: u32| -> Option<&'a str> {
            let index = u16::try_from(index).ok().filter(|&i| i != 0)?;
            configstring(CS_MODELS + index).filter(|s| !s.is_empty())
        };
        let body = strip_xmodel(model(field("modelindex"))?).to_string();
        let mut attachments = Vec::new();
        for i in 0..6 {
            let Some(name) = model(field(&format!("attachModelIndex[{i}]"))) else {
                continue;
            };
            let tag = field(&format!("attachTagIndex[{i}]"));
            let tag = u16::try_from(tag)
                .ok()
                .filter(|&t| t != 0)
                .and_then(|t| configstring(CS_TAGS + t))
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            attachments.push((strip_xmodel(name).to_string(), tag));
        }
        Some(Character {
            body,
            attachments,
            ignore_collision: 0,
        })
    }

    /// The stock `character/<name>.gsc`, read as [`parse_character`] reads
    /// it.
    pub fn load(fs: &dyn AssetSource, name: &str) -> Result<Character> {
        let path = format!("character/{name}.gsc");
        let text = fs.read(&path).ok_or_else(|| anyhow!("{path} not found"))?;
        parse_character(&String::from_utf8_lossy(&text), |alias| {
            xmodel_alias(fs, alias)
        })
        .with_context(|| path)
    }

    /// vcod's stand-in for a client whose state names no body yet: the
    /// first character of the stock Normandy mptype of its side,
    /// `mp_german_normandy` for axis and free (team 0 and 1),
    /// `mp_american_normandy_braeburn` for allies (2). The stock
    /// `mp_axis_default` and `mp_allies_default` name bodies no pak ships
    /// (`playerbody_german_wehrmacht`, `playerbody_american_ranger`). Retail
    /// draws no body it has no model for; this is ours.
    pub fn default_for_team(fs: &dyn AssetSource, team: i32) -> Result<Character> {
        let mptype = if team == 2 {
            "american_normandy"
        } else {
            "german_normandy"
        };
        let first = mptype_characters(fs, mptype)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("mptype/{mptype}.gsc picks no character"))?;
        Character::load(fs, &first)
    }
}

/// `xmodelalias/<name>.gsc`'s `a[n] = "xmodel/..."` entries in order,
/// without the prefix.
pub fn xmodel_alias(fs: &dyn AssetSource, name: &str) -> Result<Vec<String>> {
    let path = format!("xmodelalias/{name}.gsc");
    let text = fs.read(&path).ok_or_else(|| anyhow!("{path} not found"))?;
    let names: Vec<String> = String::from_utf8_lossy(&text)
        .lines()
        .filter(|l| l.contains('='))
        .filter_map(quoted)
        .map(|s| strip_xmodel(s).to_string())
        .collect();
    if names.is_empty() {
        bail!("{path}: no entries");
    }
    Ok(names)
}

/// The characters `mptype/<name>.gsc`'s `main()` picks among.
pub fn mptype_characters(fs: &dyn AssetSource, name: &str) -> Result<Vec<String>> {
    let path = format!("mptype/{name}.gsc");
    let text = fs.read(&path).ok_or_else(|| anyhow!("{path} not found"))?;
    let text = String::from_utf8_lossy(&text);
    let main = text.split("precache()").next().unwrap_or("");
    Ok(main
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("character\\")?;
            Some(rest.split("::").next()?.to_string())
        })
        .collect())
}

/// The first `"..."` on a line.
fn quoted(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let len = line[start..].find('"')?;
    Some(&line[start..start + len])
}

/// The `xmodelalias\<name>::main()` a line calls.
fn alias_call(line: &str) -> Option<&str> {
    let rest = &line[line.find("xmodelalias\\")? + "xmodelalias\\".len()..];
    rest.split("::").next()
}

/// A stock character script's `main()`, the statements the generator
/// writes: `setModel`, `setModelFromArray` and `attachFromArray` over an
/// alias, `self.hatModel = ...` and `attach`. Where retail picks an alias
/// entry at random (`randomElement`, the two `FromArray` helpers) this takes
/// the first, so a load is deterministic. `alias` resolves an alias name to
/// its entries.
pub fn parse_character(
    text: &str,
    alias: impl Fn(&str) -> Result<Vec<String>>,
) -> Result<Character> {
    let main = text
        .split("precache()")
        .next()
        .ok_or_else(|| anyhow!("no main()"))?;
    let first = |line: &str| -> Result<String> {
        let name = alias_call(line).ok_or_else(|| anyhow!("no alias in `{line}`"))?;
        alias(name)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("alias {name} is empty"))
    };
    let mut body = None;
    let mut hat = None;
    let mut attachments = Vec::new();
    for line in main.lines().map(str::trim) {
        if line.starts_with("//") || line.is_empty() {
            continue;
        }
        if line.contains("setModelFromArray(") {
            body = Some(first(line)?);
        } else if line.contains("setModel(") {
            body = quoted(line).map(|s| strip_xmodel(s).to_string());
        } else if line.contains("attachFromArray(") {
            attachments.push((first(line)?, None));
        } else if line.starts_with("self.hatModel") {
            hat = Some(match quoted(line) {
                Some(s) => strip_xmodel(s).to_string(),
                None => first(line)?,
            });
        } else if let Some(args) = line.strip_prefix("self attach(") {
            let mut args = args.split(',').map(str::trim);
            let model = args.next().unwrap_or("");
            let model = if model == "self.hatModel" {
                hat.clone()
                    .ok_or_else(|| anyhow!("attach of an unset hatModel"))?
            } else {
                strip_xmodel(quoted(model).ok_or_else(|| anyhow!("bad attach `{line}`"))?)
                    .to_string()
            };
            let tag = args
                .next()
                .and_then(quoted)
                .filter(|t| !t.is_empty())
                .map(str::to_string);
            attachments.push((model, tag));
        }
    }
    let body = body.ok_or_else(|| anyhow!("no setModel in main()"))?;
    Ok(Character {
        body,
        attachments,
        ignore_collision: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"// THIS FILE IS AUTOGENERATED, DO NOT MODIFY
main()
{
	codescripts\character::setModelFromArray(xmodelalias\mp_body_german_africa::main());
	codescripts\character_mp::attachFromArray(xmodelalias\mp_head_german_africa::main());
	self.hatModel = "xmodel/helmet_german_africa";
	self attach(self.hatModel, "", true);
	self attach("xmodel/gear_test", "j_spine4", true);
	self setViewmodel("xmodel/viewmodel_hands_british_bare");
}

precache()
{
	precacheModel("xmodel/not_in_main");
}
"#;

    #[test]
    fn reads_a_generated_character_script() {
        let c = parse_character(SCRIPT, |name| {
            Ok(match name {
                "mp_body_german_africa" => vec!["playerbody_german_africa01".into()],
                "mp_head_german_africa" => vec!["head_a".into(), "head_b".into()],
                _ => bail!("unknown alias {name}"),
            })
        })
        .unwrap();
        assert_eq!(c.body, "playerbody_german_africa01");
        assert_eq!(
            c.attachments,
            [
                ("head_a".to_string(), None),
                ("helmet_german_africa".to_string(), None),
                ("gear_test".to_string(), Some("j_spine4".to_string())),
            ]
        );
    }

    #[test]
    fn a_client_state_names_its_models_through_the_configstrings() {
        let mut cs = ClientState2::null(0);
        let set = |cs: &mut ClientState2, name: &str, v: u32| {
            cs.f[field_index(&CLIENT_STATE_FIELDS, name)] = v;
        };
        // The retail capture's client 0: body 72, head 73, helmet 76.
        set(&mut cs, "modelindex", 72);
        set(&mut cs, "attachModelIndex[0]", 73);
        set(&mut cs, "attachModelIndex[1]", 76);
        set(&mut cs, "attachModelIndex[2]", 77);
        set(&mut cs, "attachTagIndex[2]", 1);
        let strings = |i: u16| match i {
            406 => Some("xmodel/playerbody_german_africa01"),
            407 => Some("xmodel/head_german_afrca_christoph"),
            410 => Some("xmodel/helmet_german_africa"),
            411 => Some("xmodel/gear"),
            111 => Some("j_spine4"),
            _ => None,
        };
        let c = Character::from_client_state(&cs, strings).unwrap();
        assert_eq!(c.body, "playerbody_german_africa01");
        assert_eq!(
            c.attachments,
            [
                ("head_german_afrca_christoph".to_string(), None),
                ("helmet_german_africa".to_string(), None),
                ("gear".to_string(), Some("j_spine4".to_string())),
            ]
        );
        set(&mut cs, "modelindex", 0);
        assert_eq!(Character::from_client_state(&cs, strings), None);
    }
}
