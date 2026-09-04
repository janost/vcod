//! The hit skeleton a locational trace walks: body plus head plus helmet,
//! grafted the way the client draws it, cached per assembly, and the xanim
//! clips it is posed with. `docs/research/cod11-combat.md` section 3.

use std::collections::HashMap;
use std::rc::Rc;
use vcod_common::pk3::Pk3Fs;
use vcod_common::skeleton::Skeleton;
use vcod_common::xanim::{self, XAnim};
use vcod_common::xmodel;

/// The models a client wears: the body, then each attachment with the tag it
/// grafts at. Mirrored off the script host with the roster, since a head and
/// a helmet are attachments the character script asks for and not part of
/// the body.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Assembly {
    pub body: String,
    pub attachments: Vec<(String, Option<String>)>,
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
        k
    }
}

/// A client's model name as script wrote it, minus the `xmodel/` prefix
/// `setModel` and `attach` are given.
pub fn model_name(raw: &str) -> String {
    raw.trim_start_matches("xmodel/")
        .trim_start_matches("xmodel\\")
        .to_string()
}

/// Grafted skeletons and anim clips, loaded on first use and kept for the
/// map. A shot is rare next to a frame, so nothing here is warmed up front.
#[derive(Default)]
pub struct HitRigs {
    rigs: HashMap<String, Option<Rc<Skeleton>>>,
    clips: HashMap<String, Option<Rc<XAnim>>>,
}

impl HitRigs {
    /// The grafted skeleton for an assembly. `None` when the body will not
    /// load, which leaves the caller with no hit location rather than no hit.
    pub fn rig(&mut self, fs: &Pk3Fs, assembly: &Assembly) -> Option<Rc<Skeleton>> {
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

    /// One anim clip, cached by name. `None` for a clip the paks lack.
    pub fn clip(&mut self, fs: &Pk3Fs, name: &str) -> Option<Rc<XAnim>> {
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

/// An attachment that will not load is left out, the way the client leaves it
/// undrawn; a body that will not load has no rig at all.
fn build(fs: &Pk3Fs, assembly: &Assembly) -> Option<Rc<Skeleton>> {
    let body = xmodel::load(fs, &assembly.body).ok()?;
    let parts: Vec<(xmodel::XModel, Option<&str>)> = assembly
        .attachments
        .iter()
        .filter_map(|(name, tag)| Some((xmodel::load(fs, name).ok()?, tag.as_deref())))
        .collect();
    let mut refs: Vec<(&xmodel::XModel, Option<&str>)> = vec![(&body, None)];
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
        };
        let mut b = a.clone();
        b.attachments.swap(0, 1);
        assert_ne!(a.key(), b.key());
        let mut c = a.clone();
        c.attachments[0].1 = Some("tag_head".into());
        assert_ne!(a.key(), c.key());
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
        let Some(fs) = vcod_common::testing::game_fs() else {
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
}
