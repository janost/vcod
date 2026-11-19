//! The hit skeleton a locational trace walks: body plus head plus helmet,
//! grafted the way the client draws it, cached per assembly, and the xanim
//! clips it is posed with. `docs/research/cod11-combat.md` section 3.

use std::collections::HashMap;
use std::rc::Rc;
use vcod_common::asset_source::AssetSource;
use vcod_common::model;
use vcod_common::skeleton::Skeleton;
use vcod_common::xanim::{self, XAnim};

/// The models a client wears: the body, then each attachment with the tag it
/// grafts at. Mirrored off the script host with the roster, since a head and
/// a helmet are attachments the character script asks for and not part of
/// the body.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Assembly {
    pub body: String,
    pub attachments: Vec<(String, Option<String>)>,
    /// 1.5's `attach(model, tag, ignoreCollision)`, a bit per attachment
    /// (`gentity+0x17a`): such a model hangs on the skeleton but its boxes
    /// take no hits.
    pub ignore_collision: u8,
}

impl Assembly {
    /// Cache key; `name@tag` per attachment, in slot order.
    fn key(&self) -> String {
        let mut k = self.body.clone();
        for (name, tag) in &self.attachments {
            k.push(';');
            k.push_str(name);
            if let Some(tag) = tag {
                k.push('@');
                k.push_str(tag);
            }
        }
        if self.ignore_collision != 0 {
            k.push_str(&format!("#{}", self.ignore_collision));
        }
        k
    }
}

/// A player's DObj across `G_UpdateClientInfo`s: the models and the
/// ignoreCollision bits it was built with, which hold until a rebuild
/// (1.5b `BG_UpdatePlayerDObj` 0x1afc9, UO 0x1f556;
/// cod15-protocol-diff.md, `getAttachIgnoreCollision`).
#[derive(Clone, Default, Debug)]
pub struct PlayerDObj {
    /// The client info's model and tag names, copied off the entity at each
    /// update; an empty slot clears its names without counting as a change.
    names: Assembly,
    /// What the DObj was built from: the hit walk's assembly.
    built: Assembly,
    /// The weapon it was built with, 0 for a rider without one.
    weapon: i32,
    /// The client info's rebuild flag (1.5b `+0x3f8`, UO `+0x404`).
    rebuild: bool,
    /// The torso lerp frame's anim, toggle bit and all.
    torso: i32,
}

impl PlayerDObj {
    /// One `G_UpdateClientInfo` (the end frame's, and on UO every
    /// `G_DObjUpdate`'s): `now` is what the entity wears, `weapon` its
    /// weapon for the DObj, `torso` its `torsoAnim`. A changed name frees
    /// the DObj; a free one, a weapon other than the built one or a set flag
    /// rebuilds it, reading the bits as they are. Then `BG_PlayerAnimation`:
    /// a new torso anim other than 0, toggle bit aside, sets the flag for the
    /// next update (`BG_SetNewAnimation`, 1.5b 0x1908f, UO 0x1d201).
    pub fn update(&mut self, now: &Assembly, weapon: i32, torso: i32) -> &Assembly {
        let changed = now.body != self.names.body
            || now
                .attachments
                .iter()
                .enumerate()
                .any(|(i, a)| self.names.attachments.get(i) != Some(a));
        self.names = now.clone();
        if changed || weapon != self.weapon || self.rebuild {
            self.built = now.clone();
            self.weapon = weapon;
            self.rebuild = false;
        }
        if torso != self.torso {
            self.torso = torso;
            self.rebuild |= torso & !0x200 != 0;
        }
        &self.built
    }
}

/// A client's model name as script wrote it, minus the `xmodel/` prefix
/// `setModel` and `attach` are given.
pub fn model_name(raw: &str) -> String {
    raw.trim_start_matches("xmodel/")
        .trim_start_matches("xmodel\\")
        .to_string()
}

/// Grafted skeletons and anim clips, loaded on first use and kept while the
/// owner lives (the CoD1 server's for the whole process). A shot is rare
/// next to a frame, so nothing here is warmed up front.
#[derive(Default)]
pub struct HitRigs {
    rigs: HashMap<String, Option<Rc<Skeleton>>>,
    clips: HashMap<String, Option<Rc<XAnim>>>,
    roots: HashMap<String, Option<String>>,
}

impl HitRigs {
    /// The grafted skeleton for an assembly. `None` when the body will not
    /// load, which leaves the caller with no hit location rather than no hit.
    pub fn rig(&mut self, fs: &dyn AssetSource, assembly: &Assembly) -> Option<Rc<Skeleton>> {
        // A client between its spawn and the frame that mirrors the character
        // script's model has no body yet; that is not a load failure.
        if assembly.body.is_empty() {
            return None;
        }
        let key = assembly.key();
        if let Some(hit) = self.rigs.get(&key) {
            return hit.clone();
        }
        let built = build(fs, assembly);
        if built.is_none() {
            log::warn!("hit rig '{key}': the body model would not load, hits read no location");
        }
        self.rigs.insert(key, built.clone());
        built
    }

    /// A bone of the assembly's rig by name, case-blind, as
    /// `DObjGetBoneIndex` finds it: the first of that name, else an
    /// attachment's own root bone, which the rig merged into the bone the
    /// attachment hangs on (cod11-movers.md 16, "A tag on an attached model").
    pub fn bone(
        &mut self,
        fs: &dyn AssetSource,
        assembly: &Assembly,
        skel: &Skeleton,
        tag: &str,
    ) -> Option<usize> {
        let find = |name: &str| {
            skel.bones()
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(name))
        };
        if let Some(i) = find(tag) {
            return Some(i);
        }
        for (name, graft) in &assembly.attachments {
            let Some(root) = self.root(fs, name) else {
                continue;
            };
            if root.eq_ignore_ascii_case(tag) {
                return find(graft.as_deref().unwrap_or(&root));
            }
        }
        None
    }

    /// An xmodel's root bone name, cached by model. `None` for a model the
    /// paks lack.
    fn root(&mut self, fs: &dyn AssetSource, name: &str) -> Option<String> {
        self.roots
            .entry(name.to_string())
            .or_insert_with(|| {
                let bones = model::load_bones(fs, name).ok()?;
                bones.first().map(|b| b.name.clone())
            })
            .clone()
    }

    /// One anim clip, cached by name. `None` for a clip the paks lack.
    pub fn clip(&mut self, fs: &dyn AssetSource, name: &str) -> Option<Rc<XAnim>> {
        if let Some(hit) = self.clips.get(name) {
            return hit.clone();
        }
        let loaded = match xanim::load(fs, name) {
            Ok(a) => Some(Rc::new(a)),
            Err(e) => {
                log::warn!("hit rig clip '{name}': {e:#}");
                None
            }
        };
        self.clips.insert(name.to_string(), loaded.clone());
        loaded
    }
}

/// A model's skeleton alone: a rig reads the bones and the name, never the
/// surfaces, so no `xmodelsurfs` is parsed or baked.
fn bones_only(fs: &dyn AssetSource, name: &str) -> anyhow::Result<model::Model> {
    Ok(model::Model {
        name: name.to_string(),
        bones: model::load_bones(fs, name)?,
        lods: Vec::new(),
        collision: Vec::new(),
    })
}

/// An attachment that will not load is left out, the way the client leaves it
/// undrawn; a body that will not load has no rig at all.
fn build(fs: &dyn AssetSource, assembly: &Assembly) -> Option<Rc<Skeleton>> {
    let body = bones_only(fs, &assembly.body).ok()?;
    let parts: Vec<(model::Model, Option<&str>)> = assembly
        .attachments
        .iter()
        .enumerate()
        .filter_map(|(i, (name, tag))| {
            let mut model = bones_only(fs, name).ok()?;
            // The engine's hit-box walk skips a flagged model's boxes
            // (docs/research/cod15-protocol-diff.md, `getAttachIgnoreCollision`).
            if assembly.ignore_collision >> i & 1 != 0 {
                for b in &mut model.bones {
                    b.hit_mins = glam::Vec3::ZERO;
                    b.hit_maxs = glam::Vec3::ZERO;
                }
            }
            Some((model, tag.as_deref()))
        })
        .collect();
    let mut refs: Vec<(&model::Model, Option<&str>)> = vec![(&body, None)];
    refs.extend(parts.iter().map(|(m, t)| (m, *t)));
    Some(Rc::new(Skeleton::build_grafted(&refs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_separates_tags_and_slot_order() {
        let a = Assembly {
            body: "b".into(),
            attachments: vec![
                ("head".into(), None),
                ("hat".into(), Some("tag_helmet".into())),
            ],
            ignore_collision: 0,
        };
        let mut b = a.clone();
        b.attachments.swap(0, 1);
        assert_ne!(a.key(), b.key());
        let mut c = a.clone();
        c.attachments[0].1 = Some("tag_head".into());
        assert_ne!(a.key(), c.key());
        let mut d = a.clone();
        d.ignore_collision = 1;
        assert_ne!(a.key(), d.key());
    }

    /// Retail 1.5b (`client-probes/probe_ignorecoll`): the same models
    /// with other bits keep the built bits until a weapon change or a new
    /// torso anim; a changed model takes them at once.
    #[test]
    fn a_player_dobj_keeps_its_bits_until_a_rebuild() {
        let wear = |h: &str, bits| Assembly {
            body: "b".into(),
            attachments: vec![(h.into(), None), ("hat".into(), None)],
            ignore_collision: bits,
        };
        let mut d = PlayerDObj::default();
        let mut bits = |a: Assembly, weapon, torso| d.update(&a, weapon, torso).ignore_collision;
        assert_eq!(bits(wear("h1", 3), 0, 0), 3);
        assert_eq!(bits(wear("h1", 0), 0, 0), 3);
        assert_eq!(bits(wear("h2", 0), 0, 0), 0);
        assert_eq!(bits(wear("h2", 1), 0, 0), 0);
        assert_eq!(bits(wear("h2", 1), 5, 0), 1);
        // The toggle bit alone flags nothing.
        assert_eq!(bits(wear("h2", 2), 5, 0x200), 1);
        assert_eq!(bits(wear("h2", 2), 5, 0x200), 1);
        // A new torso anim rebuilds at the update after its own.
        assert_eq!(bits(wear("h2", 2), 5, 7), 1);
        assert_eq!(bits(wear("h2", 2), 5, 7), 2);
    }

    /// An emptied slot clears its names without a rebuild: the detached
    /// model stays in the DObj until one. UO's detachAll and attach each
    /// update, so a re-attach of the same models rebuilds there.
    #[test]
    fn a_detach_alone_keeps_the_built_models() {
        let mut d = PlayerDObj::default();
        let both = Assembly {
            body: "b".into(),
            attachments: vec![("head".into(), None), ("hat".into(), None)],
            ignore_collision: 0,
        };
        let mut head = both.clone();
        head.attachments.pop();
        d.update(&both, 0, 0);
        assert_eq!(d.update(&head, 0, 0), &both);
        let mut flagged = both.clone();
        flagged.ignore_collision = 2;
        assert_eq!(d.update(&flagged, 0, 0), &flagged);
        d.update(&head, 0, 0);
        assert_eq!(d.update(&head, 4, 0), &head);
    }

    #[test]
    fn model_names_lose_the_xmodel_prefix() {
        assert_eq!(model_name("xmodel/playerbody_x"), "playerbody_x");
        assert_eq!(model_name("xmodel\\playerbody_x"), "playerbody_x");
        assert_eq!(model_name("playerbody_x"), "playerbody_x");
    }

    /// The stock player assembly grafts to one skeleton whose head and helmet
    /// bones carry the boxes the body's own do not.
    #[test]
    fn the_stock_assembly_has_a_hittable_head() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let mut rigs = HitRigs::default();
        let skel = rigs
            .rig(
                &fs,
                &Assembly {
                    body: "playerbody_american_airborne".into(),
                    attachments: vec![
                        ("basehead2".into(), None),
                        ("USAirborneHelmet".into(), Some("tag_helmet".into())),
                    ],
                    ignore_collision: 0,
                },
            )
            .expect("the stock body loads");
        let of = |name: &str| {
            let b = &skel.bones()[skel.bone_index(name).unwrap()];
            (b.has_hit_box(), b.hit_location)
        };
        assert_eq!(of("bip01 head"), (true, 2));
        assert_eq!(of("bip01 neck"), (true, 3));
        assert_eq!(of("back_up"), (true, 4)); // torso_upper
        assert_eq!(of("back_low"), (true, 5)); // torso_lower
        assert_eq!(of("bip01 l thigh"), (true, 13)); // left_leg_upper
        assert_eq!(of("tag_weapon_right"), (false, 18)); // gun, and no box
    }

    /// An attachment flagged `ignoreCollision` hangs on the skeleton without
    /// its boxes: the head ignored, the head bone takes no hit.
    #[test]
    fn an_ignored_attachment_brings_no_boxes() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut rigs = HitRigs::default();
        let mut a = Assembly {
            body: "playerbody_american_airborne".into(),
            attachments: vec![
                ("basehead2".into(), None),
                ("USAirborneHelmet".into(), Some("tag_helmet".into())),
            ],
            ignore_collision: 0,
        };
        let head = |rigs: &mut HitRigs, a: &Assembly| {
            let skel = rigs.rig(&fs, a).expect("the stock body loads");
            skel.bones()[skel.bone_index("bip01 head").unwrap()].has_hit_box()
        };
        assert!(head(&mut rigs, &a));
        a.ignore_collision = 1;
        assert!(!head(&mut rigs, &a));
    }
}
