//! Animtree (`.atr`) parser and the player anim index table.
//!
//! `legsAnim`/`torsoAnim` are 10-bit: index = `value & 511`, bit 512 is the
//! restart toggle. The index order is not the file order; see
//! docs/research/player-model-anim-system.md, "Animation indices: the animtree".

use crate::asset_source::AssetSource;
use anyhow::{Result, anyhow, bail};
use std::collections::{HashMap, HashSet};

pub struct Node {
    pub name: String,
    /// Into [`AnimTree::nodes`]; empty means a leaf.
    pub children: Vec<usize>,
    /// `: complete loopsync`, as opposed to `nonloopsync`.
    pub loopsync: bool,
    /// Either `loopsync` or `nonloopsync`: a node whose descendants all take
    /// its own time, where a plain node's leaves each step their own
    /// (cod11-combat.md 3.1, "The clip's float time").
    pub sync: bool,
    /// `: complete`: a group the engine blends its children under, the
    /// MG42 aim groups and UO's vehicle seat groups.
    pub complete: bool,
}

/// File order. `nodes[0]` is a synthetic `root` over the top-level blocks.
pub struct AnimTree {
    pub nodes: Vec<Node>,
}

/// Wire `legsAnim`/`torsoAnim` value to node id in an [`AnimTree`].
pub struct AnimIndex {
    entries: Vec<usize>,
}

#[derive(PartialEq)]
enum Tok<'a> {
    Open,
    Close,
    Word(&'a str),
}

/// Keeps newlines so a line-oriented scan of the result still lines up.
pub(crate) fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("*/") {
            Some(end) => {
                out.extend(after[..end].chars().filter(|&c| c == '\n'));
                rest = &after[end + 2..];
            }
            None => rest = "",
        }
    }
    out.push_str(rest);
    out.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tokenize(text: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    for chunk in text.split_whitespace() {
        let mut rest = chunk;
        while let Some(i) = rest.find(['{', '}']) {
            if i > 0 {
                out.push(Tok::Word(&rest[..i]));
            }
            out.push(if rest.as_bytes()[i] == b'{' {
                Tok::Open
            } else {
                Tok::Close
            });
            rest = &rest[i + 1..];
        }
        if !rest.is_empty() {
            out.push(Tok::Word(rest));
        }
    }
    out
}

impl AnimTree {
    pub fn parse(text: &str) -> Result<AnimTree> {
        let text = strip_comments(text);
        let toks = tokenize(&text);
        let mut tree = AnimTree {
            nodes: vec![Node {
                name: "root".into(),
                children: Vec::new(),
                loopsync: false,
                sync: false,
                complete: false,
            }],
        };
        let mut i = 0;
        tree.parse_block(&toks, &mut i, 0, true, 0)?;
        if tree.nodes[0].children.is_empty() {
            bail!("animtree has no blocks");
        }
        Ok(tree)
    }

    /// `depth` is bounded by `MAX_NEST_DEPTH` so a hostile `.atr` (a downloaded
    /// pk3 can shadow the stock one) errors instead of overflowing the stack.
    /// The shipped tree nests 4 deep.
    fn parse_block(
        &mut self,
        toks: &[Tok],
        i: &mut usize,
        parent: usize,
        top_level: bool,
        depth: usize,
    ) -> Result<()> {
        const MAX_NEST_DEPTH: usize = 64;
        if depth > MAX_NEST_DEPTH {
            bail!("animtree nesting depth exceeded");
        }
        loop {
            match toks.get(*i) {
                None if top_level => return Ok(()),
                None => bail!("unterminated block in animtree"),
                Some(Tok::Close) => {
                    if top_level {
                        bail!("unbalanced `}}` in animtree");
                    }
                    *i += 1;
                    return Ok(());
                }
                Some(Tok::Open) => bail!("unexpected `{{` in animtree"),
                Some(Tok::Word(name)) => {
                    let name = (*name).to_string();
                    *i += 1;
                    // Optional `: complete loopsync` modifier list before `{`.
                    let mut loopsync = false;
                    let mut sync = false;
                    let mut complete = false;
                    while let Some(Tok::Word(w)) = toks.get(*i) {
                        let is_mod = matches!(*w, ":" | "complete" | "loopsync" | "nonloopsync");
                        if !is_mod && !w.starts_with(':') {
                            break;
                        }
                        loopsync |= w.contains("loopsync") && !w.contains("nonloopsync");
                        sync |= w.contains("loopsync");
                        complete |= w.contains("complete");
                        *i += 1;
                    }
                    let idx = self.nodes.len();
                    self.nodes.push(Node {
                        name,
                        children: Vec::new(),
                        loopsync,
                        sync,
                        complete,
                    });
                    self.nodes[parent].children.push(idx);
                    if toks.get(*i) == Some(&Tok::Open) {
                        *i += 1;
                        self.parse_block(toks, i, idx, false, depth + 1)?;
                    }
                }
            }
        }
    }

    pub fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
    }

    pub fn load(fs: &dyn AssetSource, name: &str) -> Result<AnimTree> {
        let path = format!("animtrees/{name}.atr");
        let data = fs
            .read(&path)
            .ok_or_else(|| anyhow!("{path} not found in pk3s"))?;
        AnimTree::parse(&String::from_utf8_lossy(&data))
    }
}

impl AnimIndex {
    /// Flattens `tree` into the engine's animation array. Leaves that
    /// `referenced` (from [`crate::animscript::AnimScript::referenced_names`])
    /// does not name are dropped, as the engine drops anims whose names were
    /// never interned. Verified against a live 1.1d server's `animations[]`
    /// (research doc, "Animation indices: the animtree").
    pub fn build(tree: &AnimTree, referenced: &HashSet<String>) -> AnimIndex {
        let mut entries = vec![0usize];
        expand(tree, 0, false, referenced, &mut entries);
        AnimIndex { entries }
    }

    /// Masks off the restart toggle bit.
    pub fn node_id(&self, wire: i32) -> Option<usize> {
        self.entries.get((wire & 511) as usize).copied()
    }

    pub fn node<'a>(&self, tree: &'a AnimTree, wire: i32) -> Option<&'a Node> {
        tree.node(self.node_id(wire)?)
    }

    pub fn name<'a>(&self, tree: &'a AnimTree, wire: i32) -> Option<&'a str> {
        self.node(tree, wire).map(|n| n.name.as_str())
    }

    /// The wire value that plays `name`, folded because the engine folds anim
    /// names. The first index that carries the name wins; the flattening
    /// gives a node exactly one slot.
    pub fn wire_of(&self, tree: &AnimTree, name: &str) -> Option<i32> {
        let name = name.to_ascii_lowercase();
        self.entries
            .iter()
            .position(|&id| tree.nodes[id].name.to_ascii_lowercase() == name)
            .map(|i| i as i32)
    }

    /// Slot 0 is always the root, so there is no empty index and no `is_empty`.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Appends `parent`'s kept children in reverse file order, then expands each.
/// `anc_ref`: an ancestor is script-referenced, which keeps the whole subtree.
/// A node with children is kept when something under it is referenced: UO's
/// unreferenced `vehicleJeepGunnerDriving` group takes no index on the
/// retail server (its captures' binocular and prone indices,
/// docs/research/coduo-diff.md 20); 1.1's tree has no such group.
fn expand(
    tree: &AnimTree,
    parent: usize,
    anc_ref: bool,
    referenced: &HashSet<String>,
    out: &mut Vec<usize>,
) {
    let is_ref = |id: usize| referenced.contains(&tree.nodes[id].name.to_ascii_lowercase());
    fn under_ref(tree: &AnimTree, id: usize, is_ref: &dyn Fn(usize) -> bool) -> bool {
        tree.nodes[id]
            .children
            .iter()
            .any(|&c| is_ref(c) || under_ref(tree, c, is_ref))
    }
    let kept: Vec<usize> = tree.nodes[parent]
        .children
        .iter()
        .rev()
        .copied()
        .filter(|&c| anc_ref || is_ref(c) || under_ref(tree, c, &is_ref))
        .collect();
    out.extend(kept.iter().copied());
    for c in kept {
        expand(tree, c, anc_ref || is_ref(c), referenced, out);
    }
}

/// The three things a player's animation needs: the tree, the wire index into
/// it, and the script that picks a name.
pub struct PlayerAnims {
    pub tree: AnimTree,
    pub index: AnimIndex,
    pub script: crate::animscript::AnimScript,
    /// Each `EVENTS` anim's own length in ms, from the xanim header. An event
    /// clause without a `duration` holds its channel for this
    /// (`animscript::AnimState::event`); a name the paks carry no clip for is
    /// not in here and holds nothing.
    lengths: HashMap<String, u32>,
    /// Each state anim's record flags (`AnimScript::anim_records`).
    records: HashMap<String, crate::animscript::AnimRecord>,
    /// Each wire index's clip timing and `moveSpeed`, `None` for a container
    /// or a name the paks carry no clip for.
    motions: Vec<Option<AnimMotion>>,
    /// [`Self::leaf_motion`]'s table, by node id.
    leaf_motions: HashMap<usize, (f32, bool)>,
    /// The rate divides by [`AnimMotion::move_speed_f`], CoD2's float
    /// `moveSpeed`, not the truncated one; CoD2's loaders set it.
    pub float_move_speed: bool,
}

/// The leaves under a `complete` node that are not under a sync one: each
/// steps its own node time (cod11-combat.md 3.1, "The clip's float time").
fn own_time_leaves(tree: &AnimTree) -> Vec<usize> {
    fn walk(tree: &AnimTree, n: usize, under: bool, out: &mut Vec<usize>) {
        let node = &tree.nodes[n];
        if node.sync {
            return;
        }
        if node.children.is_empty() {
            if under {
                out.push(n);
            }
            return;
        }
        for &c in &node.children {
            walk(tree, c, under || node.complete, out);
        }
    }
    let mut out = Vec::new();
    walk(tree, 0, false, &mut out);
    out
}

/// What `BG_FinalizePlayerAnims` (`game.mp.i386.so` 0x27d70) keeps of a clip
/// that the DObj's clock reads (`crate::animclock`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimMotion {
    /// The loop period in seconds.
    pub duration: f32,
    /// `XAnimGetLength`, [`crate::xanim::RootMotion::length_ms`].
    pub length_ms: i32,
    pub looping: bool,
    /// [`crate::xanim::RootMotion::frequency`].
    pub frequency: f32,
    /// `animation_t +0x44`: the root's whole travel, truncated, in units per
    /// second of the clip's truncated ms length (0x27f59..0x27fa5).
    pub move_speed: i32,
    /// CoD2's `animation_t +0x44`, a float: the root's travel over the
    /// clip's float length (lnxded `BG_LoadAnim` fill 0x80d72e6), 268.47
    /// for `pb_stumble_forward` where [`Self::move_speed`] reads 268. Read
    /// in its place under [`PlayerAnims::float_move_speed`].
    pub move_speed_f: f32,
    /// `animation_t +0x50` bit 2: a `climbup` or `climbdown` anim with a
    /// `moveSpeed`, whose rate reads the vertical travel alone (0x287f5).
    pub vertical: bool,
    /// `animation_t +0x40`: the blend a change into this anim takes in place
    /// of the setter's own, 30 for a `FIREWEAPON` anim (0x2888d) and 0 for
    /// none (the engine's -1).
    pub blend_ms: i32,
    /// `animation_t +0x50` bit 0x40: a `DEATH` anim (0x288bb), whose
    /// `moveSpeed` the parser zeroes (0x288a9).
    pub death: bool,
}

impl PlayerAnims {
    pub fn load(fs: &dyn AssetSource) -> Result<PlayerAnims> {
        let tree = AnimTree::load(fs, "multiplayer")?;
        let text = fs
            .read("mp/playeranim.script")
            .ok_or_else(|| anyhow!("mp/playeranim.script not found in pk3s"))?;
        let script = crate::animscript::AnimScript::parse(&String::from_utf8_lossy(&text))?;
        let index = AnimIndex::build(&tree, &script.referenced_names());
        let lengths = script
            .event_referenced_names()
            .iter()
            .filter_map(|name| {
                let ms = match fs.read(&format!("xanim/{name}")) {
                    Some(data) => crate::xanim::duration_ms(name, &data).ok()?,
                    None => fs.decoded_anim(name)?.ok()?.motion().length_ms as u32,
                };
                Some((name.clone(), ms))
            })
            .collect();
        let records = script.anim_records();
        let climb = crate::animscript::movetype_bit("climbup")
            | crate::animscript::movetype_bit("climbdown");
        // `BG_ParseCommands` flags every anim line under these two events
        // (0x28868..0x288bb), after `moveSpeed` was computed.
        let event_anims = |event: &str| -> HashSet<String> {
            script.event(event).map_or_else(HashSet::new, |b| {
                b.clauses
                    .iter()
                    .flat_map(|c| c.legs.iter().chain(&c.torso))
                    .map(|a| a.name.to_ascii_lowercase())
                    .collect()
            })
        };
        let (deaths, fires) = (event_anims("death"), event_anims("fireweapon"));
        let motions = (0..index.len() as i32)
            .map(|wire| {
                let node = index.node(&tree, wire)?;
                if !node.children.is_empty() {
                    return None;
                }
                let m = crate::xanim::clip_motion(fs, &node.name)?;
                let travel = m.distance as i32;
                let move_speed = if m.length_ms != 0 && travel != 0 {
                    travel * 1000 / m.length_ms
                } else {
                    0
                };
                let movetypes = records
                    .get(&node.name.to_ascii_lowercase())
                    .map_or(0, |r| r.movetypes);
                let name = node.name.to_ascii_lowercase();
                let death = deaths.contains(&name);
                let move_speed = if death { 0 } else { move_speed };
                let move_speed_f = if death || m.duration == 0.0 {
                    0.0
                } else {
                    m.distance / m.duration
                };
                Some(AnimMotion {
                    duration: m.duration,
                    length_ms: m.length_ms,
                    looping: m.looping,
                    frequency: m.frequency,
                    move_speed,
                    move_speed_f,
                    vertical: move_speed != 0 && movetypes & climb != 0,
                    blend_ms: if fires.contains(&name) { 30 } else { 0 },
                    death,
                })
            })
            .collect();
        let leaf_motions = own_time_leaves(&tree)
            .into_iter()
            .filter_map(|leaf| {
                let name = &tree.nodes[leaf].name;
                let m = crate::xanim::clip_motion(fs, name)?;
                Some((leaf, (m.frequency, m.looping)))
            })
            .collect();
        Ok(PlayerAnims {
            tree,
            index,
            records,
            script,
            lengths,
            motions,
            leaf_motions,
            float_move_speed: false,
        })
    }

    /// An event anim's own length in ms, or `None` for a name the paks carry
    /// no clip for.
    pub fn length_ms(&self, name: &str) -> Option<u32> {
        self.lengths.get(name).copied()
    }

    pub fn name(&self, wire: i32) -> Option<&str> {
        self.index.name(&self.tree, wire)
    }

    /// The clip timing and `moveSpeed` of the anim a wire value plays.
    pub fn motion(&self, wire: i32) -> Option<AnimMotion> {
        self.motions.get((wire & 511) as usize).copied().flatten()
    }

    /// [`crate::xanim::RootMotion::frequency`] and looping of a leaf that
    /// steps a node time of its own under a `complete` group with no sync
    /// (a UO seat's leaves); `None` for any other node or a missing clip.
    pub fn leaf_motion(&self, node: usize) -> Option<(f32, bool)> {
        self.leaf_motions.get(&node).copied()
    }

    /// The record flags of the anim a wire value plays; default for an anim
    /// no state names.
    pub fn record(&self, wire: i32) -> crate::animscript::AnimRecord {
        self.name(wire)
            .and_then(|n| self.records.get(&n.to_ascii_lowercase()))
            .copied()
            .unwrap_or_default()
    }

    /// The wire value for an anim name, or `None` when the tree has no such
    /// node.
    pub fn wire_of(&self, name: &str) -> Option<i32> {
        self.index.wire_of(&self.tree, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
// comment
main
{
    torso
    {
        pt_a
        pt_b // trailing comment
    }
    legs
    {
        pb_walk
        grp : complete loopsync
        {
            grp_low
            {
                pb_x
                pb_y
            }
        }
    }
}
/*
main
{
    pb_commented_out
}
*/
turning
{
    pl_t
}
"#;

    #[test]
    fn parses_nested_tree_in_file_order() {
        let t = AnimTree::parse(SAMPLE).unwrap();
        let names: Vec<&str> = t.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "root", "main", "torso", "pt_a", "pt_b", "legs", "pb_walk", "grp", "grp_low",
                "pb_x", "pb_y", "turning", "pl_t"
            ]
        );
        assert!(t.nodes[7].loopsync); // grp
        assert_eq!(t.nodes[7].children, vec![8]);
        assert_eq!(t.nodes[8].children, vec![9, 10]);
        assert!(t.nodes[6].children.is_empty()); // pb_walk leaf
        assert!(t.nodes.iter().all(|n| n.name != "pb_commented_out"));
        assert_eq!(t.nodes[0].children, vec![1, 11]);
    }

    /// A plain `complete` group's leaves keep their own time; a sync
    /// group's take the group's, and a leaf under no group is the clock's.
    #[test]
    fn only_a_plain_complete_group_s_leaves_keep_their_own_time() {
        let t = AnimTree::parse(
            "main { legs { pb_walk seat : complete { seat_aim { pb_l pb_r } } \
             mg : complete nonloopsync { mg_lvl { pb_mg } } } }",
        )
        .unwrap();
        let names: Vec<&str> = own_time_leaves(&t)
            .iter()
            .map(|&n| t.nodes[n].name.as_str())
            .collect();
        assert_eq!(names, ["pb_l", "pb_r"]);
    }

    #[test]
    fn index_order_is_reversed_and_skips_unreferenced_leaves() {
        let t = AnimTree::parse(SAMPLE).unwrap();
        // pb_walk and pt_a are unreferenced; grp's subtree stays because grp is named
        let referenced: HashSet<String> = ["pt_b", "grp", "pl_t"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let idx = AnimIndex::build(&t, &referenced);
        let names: Vec<&str> = (0..idx.len() as i32)
            .map(|i| idx.name(&t, i).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "root", "turning", "main", // top-level blocks, reversed
                "pl_t", "legs", "torso", // their children, reversed
                "grp",   // legs' only kept child (pb_walk is unreferenced)
                "grp_low", "pb_y", "pb_x", // grp's subtree, kept by its parent
                "pt_b", // torso's children come after legs' whole subtree
            ]
        );
    }

    #[test]
    fn rejects_pathologically_deep_nesting() {
        let mut text = String::new();
        for _ in 0..100 {
            text.push_str("n {\n");
        }
        for _ in 0..100 {
            text.push_str("}\n");
        }
        assert!(AnimTree::parse(&text).is_err());
    }

    #[test]
    fn parses_real_multiplayer_atr() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let t = AnimTree::load(&fs, "multiplayer").unwrap();
        assert_eq!(t.nodes[0].name, "root");
        assert!(t.nodes.iter().all(|n| !n.name.contains("minorpain")));
        let aim = t.index_of("standMG42_aim").unwrap();
        assert_eq!(t.nodes[aim].children.len(), 3);
        assert!(!t.nodes[aim].loopsync); // ": complete nonloopsync"
        assert!(t.nodes[aim].sync);
        assert!(t.nodes[t.index_of("standMG42_fire").unwrap()].loopsync);
        // engine cap is 512
        assert!(t.nodes.len() < 512, "{} nodes", t.nodes.len());
    }

    /// Pairs read off the wire from a local 1.1d server; the full table matches
    /// its dumped `animations[]`.
    #[test]
    fn player_anim_index_matches_the_wire() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        assert_eq!(anims.index.len(), 254);
        for (wire, name) in [
            (0, "root"),
            (85, "pb_combatrun_forward_loop_rifles"),
            (91, "pb_combatrun_right_loop"),
            (93, "pb_combatrun_back_loop"),
            (94, "pb_combatrun_forward_loop"),
            (101, "pb_runjump_land"),
            (122, "pb_stand_alert"),
            (236, "pt_rifle_fire"),
            (253, "pt_stand_shoot"),
        ] {
            assert_eq!(anims.name(wire), Some(name), "wire index {wire}");
        }
        assert_eq!(anims.name(122 | 512), Some("pb_stand_alert"));
    }

    /// The reverse of `node_id`: the wire value that plays a named anim.
    /// Case-folded, because the engine folds anim names.
    #[test]
    fn a_name_resolves_back_to_its_wire_index() {
        let tree = AnimTree::parse(SAMPLE).unwrap();
        // The same referenced set `index_order_is_reversed_and_skips_
        // unreferenced_leaves` uses, so both tests read one tree.
        let referenced: HashSet<String> = ["pt_b", "grp", "pl_t"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let index = AnimIndex::build(&tree, &referenced);
        let wire = index.wire_of(&tree, "pt_b").expect("pt_b is indexed");
        assert_eq!(index.name(&tree, wire), Some("pt_b"));
        assert_eq!(index.wire_of(&tree, "PT_B"), Some(wire));
        // `pb_walk` is a leaf nothing references, so it is not in the array.
        assert_eq!(index.wire_of(&tree, "pb_walk"), None);
        assert_eq!(index.wire_of(&tree, "not_in_the_tree"), None);
    }

    /// Every anim the real script names must exist in the real tree. A name
    /// that does not resolve is either a parser bug or a fact about the
    /// engine we have not recorded, and either way the machine cannot play
    /// it. Needs the paks.
    #[test]
    fn every_scripted_anim_resolves_in_the_multiplayer_tree() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).expect("load the player anims");
        let mut missing: Vec<String> = anims
            .script
            .referenced_names()
            .into_iter()
            .filter(|n| anims.index.wire_of(&anims.tree, n).is_none())
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "the script names anims the animtree does not index: {missing:?}"
        );
    }

    /// The movement records `BG_FinalizePlayerAnims` would build: a run loop
    /// has a `moveSpeed` and loops, an idle has none, a ladder clip reads
    /// its rate off the vertical travel. Needs the paks.
    #[test]
    fn movement_clips_carry_a_move_speed() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).expect("load the player anims");
        let m = |name: &str| anims.motion(anims.wire_of(name).unwrap()).unwrap();
        if std::env::var("MOTION_REPORT").is_ok() {
            for w in 0..anims.index.len() as i32 {
                if let (Some(n), Some(mo)) = (anims.name(w), anims.motion(w)) {
                    println!("{w} {n} {mo:?}");
                }
            }
        }
        let run = m("pb_combatrun_forward_loop");
        assert!(run.looping && run.move_speed > 100, "{run:?}");
        assert_eq!(m("pb_stand_alert").move_speed, 0);
        let climb = m("pb_climbup");
        assert!(climb.vertical && climb.move_speed > 0, "{climb:?}");
        assert!(!run.vertical);
    }

    /// United Offensive's indices as its retail server sends them: the
    /// committed UO captures' binocular, prone, stand and LMG anims and the
    /// kar98k's fire and reload in `mp_cassino-dm-combat-uo.txt`
    /// (docs/research/coduo-diff.md 20). Runs only with `COD_DIR` at a UO
    /// install.
    #[test]
    fn uo_indices_match_the_retail_captures() {
        let Some(fs) = crate::testing::uo_fs() else {
            return;
        };
        let a = PlayerAnims::load(&fs).unwrap();
        for (name, wire) in [
            ("c_mp_binocular_ads", 85),
            ("c_mp_binocular_idle", 86),
            ("pb_prone_aim", 108),
            ("pb_crouch_alert", 174),
            ("pb_stand_alert", 185),
            ("pt_reload_stand_rifle", 311),
            ("pt_rifle_fire", 318),
            ("pt_rifle_fire_prone", 320),
            ("c_mg_lmg_idle", 341),
            ("c_mp_binocular_ads_torso", 356),
        ] {
            assert_eq!(a.index.wire_of(&a.tree, name), Some(wire), "{name}");
        }
    }
}
