//! The engine's entity area tree: what `trap_EntitiesInBox` walks, and so
//! the order a blast meets its victims in. A port of `cod_lnxded`'s
//! lazily split 2D tree (`docs/research/cod11-combat.md` 14.7), kept exact
//! down to its quirks: a new child is cut on the region the link's walk
//! handed in rather than its own half, a node pushes its one-sided entries
//! down a single level per link, a region of 512 or less never splits, and
//! an emptied node goes back to the pool. A list holds the most recently
//! prepended entry first.
//!
//! Every entity link goes through [`AreaTree::link`]; the order an entity
//! sits in a list is the history of those calls. Which entities vcod routes
//! here, and which it does not, is in the doc section.

/// The node pool past the root (`0x831b2fc`, 0x400 entries).
const POOL: usize = 0x400;
/// `MAX_GENTITIES`.
const MAX_ENTITIES: usize = crate::game::entity::MAX_GENTITIES as usize;
/// A region this wide or narrower on its split axis is never split
/// (`.rodata 0x80cd478`).
const MIN_SPLIT: f32 = 512.0;

type NodeId = u16;
const ROOT: NodeId = 0;

#[derive(Clone, Copy, Default, Debug)]
struct Node {
    axis: usize,
    dist: f32,
    /// OR of the entity contents under the node, which a query tests first.
    mask: i32,
    ents: Option<u32>,
    statics: Option<u32>,
    parent: Option<NodeId>,
    /// `+0x20`: the side above `dist`.
    above: Option<NodeId>,
    /// `+0x24`: the side below `dist`.
    below: Option<NodeId>,
}

/// One entity's place in the tree and the `gentity` fields a query reads.
#[derive(Clone, Copy, Default)]
struct EntLink {
    node: Option<NodeId>,
    next: Option<u32>,
    /// The contents and the xy box of the last link that placed it.
    stored_contents: i32,
    stored: Bounds2,
    /// `r.absmin`/`r.absmax` and `r.contents` as the last link left them.
    absmin: [f32; 3],
    absmax: [f32; 3],
    contents: i32,
}

#[derive(Clone, Copy, Default)]
struct StaticLink {
    node: NodeId,
    next: Option<u32>,
    bounds: Bounds2,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Bounds2 {
    mins: [f32; 2],
    maxs: [f32; 2],
}

/// What one `SV_LinkEntity` hands the tree: the entity's world box, its
/// contents, and the xy box it is filed under, which is the world box except
/// for a live player (see [`Link::player`]).
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub absmin: [f32; 3],
    pub absmax: [f32; 3],
    pub contents: i32,
    tree: Option<([f32; 2], [f32; 2])>,
}

impl Link {
    /// `SV_LinkEntity`'s box for a non-brush entity: the origin plus its
    /// `r.mins`/`r.maxs`, grown one unit each way (`cod_lnxded`
    /// 0x8090b60..0x8090c13).
    pub fn boxed(origin: [f32; 3], mins: [f32; 3], maxs: [f32; 3], contents: i32) -> Self {
        Link {
            absmin: std::array::from_fn(|i| origin[i] + mins[i] - 1.0),
            absmax: std::array::from_fn(|i| origin[i] + maxs[i] + 1.0),
            contents,
            tree: None,
        }
    }

    /// A playing client's link: its world box as [`Link::boxed`], filed in
    /// the tree under a fixed 128-unit square about its origin. VERIFIED:
    /// `SV_LinkEntity` takes `origin.xy` plus (-64, -64) and (64, 64)
    /// (`.data 0x80e30d0..0x80e30e0`) when the entity has a server DObj and
    /// `r.svFlags & 2`, which `ClientEndFrame` sets on a playing client
    /// (`game.mp.i386.so` 0x40fa0).
    pub fn player(origin: [f32; 3], mins: [f32; 3], maxs: [f32; 3], contents: i32) -> Self {
        Link {
            tree: Some((
                [origin[0] - 64.0, origin[1] - 64.0],
                [origin[0] + 64.0, origin[1] + 64.0],
            )),
            ..Self::boxed(origin, mins, maxs, contents)
        }
    }

    /// A brush model's link: its box at `origin` while it is unrotated, a
    /// cube of the bounds' radius about `origin` once any angle is set
    /// (`SV_LinkEntity`'s `RadiusFromBounds` call at 0x8090b20), grown a
    /// unit each way either way.
    pub fn brush(
        origin: [f32; 3],
        angles: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        contents: i32,
    ) -> Self {
        if angles == [0.0; 3] {
            return Self::boxed(origin, mins, maxs, contents);
        }
        let r = (0..3)
            .map(|i| mins[i].abs().max(maxs[i].abs()).powi(2))
            .sum::<f32>()
            .sqrt();
        Self::boxed(origin, [-r; 3], [r; 3], contents)
    }

    /// The same link filed in the tree under `mins`..`maxs` (x and y): a
    /// `script_model`'s model bounds about its origin (`svFlags & 4`,
    /// `0x80c4f6c`), where its world box stays the origin's.
    pub fn filed_under(self, mins: [f32; 2], maxs: [f32; 2]) -> Self {
        Link {
            tree: Some((mins, maxs)),
            ..self
        }
    }

    /// The same link with other contents.
    pub fn with_contents(self, contents: i32) -> Self {
        Link { contents, ..self }
    }

    fn tree_box(&self) -> Bounds2 {
        match self.tree {
            Some((mins, maxs)) => Bounds2 { mins, maxs },
            None => Bounds2 {
                mins: [self.absmin[0], self.absmin[1]],
                maxs: [self.absmax[0], self.absmax[1]],
            },
        }
    }
}

#[derive(Clone)]
pub struct AreaTree {
    nodes: Vec<Node>,
    /// Popped from the back, freed nodes pushed back on: the pool's LIFO.
    free: Vec<NodeId>,
    world: Bounds2,
    ents: Vec<EntLink>,
    statics: Vec<StaticLink>,
}

impl AreaTree {
    /// `0x8058dd0`: the root over inline model 0's xy bounds, split on the
    /// longer axis (y on a tie) at the midpoint. `world_mins`/`world_maxs`
    /// are the BSP lump's; the submodel loader (`0x804a81c`) grows them a
    /// unit each way.
    pub fn new(world_mins: [f32; 3], world_maxs: [f32; 3], max_entities: usize) -> Self {
        let world = Bounds2 {
            mins: [world_mins[0] - 1.0, world_mins[1] - 1.0],
            maxs: [world_maxs[0] + 1.0, world_maxs[1] + 1.0],
        };
        let axis = split_axis(&world);
        let root = Node {
            axis,
            dist: (world.maxs[axis] + world.mins[axis]) * 0.5,
            ..Node::default()
        };
        let mut nodes = Vec::with_capacity(POOL + 1);
        nodes.push(root);
        nodes.resize(POOL + 1, Node::default());
        AreaTree {
            nodes,
            free: (1..=POOL as NodeId).rev().collect(),
            world,
            ents: vec![EntLink::default(); max_entities],
            statics: Vec::new(),
        }
    }

    /// A tree with no map under it, for a host that loaded none: one
    /// region wide enough for any coordinate a test uses.
    pub fn unbounded() -> Self {
        Self::new([-65536.0; 3], [65536.0; 3], MAX_ENTITIES)
    }

    /// The tree a map load builds (`CM_LoadMap`, `0x804b3c0`): the root over
    /// inline model 0, then every collidable `misc_model` in entity-string
    /// order.
    pub fn for_map(bsp: &vcod_common::bsp::Bsp, fs: &vcod_common::pk3::Pk3Fs) -> Self {
        let Some(world) = bsp.models.first() else {
            return Self::unbounded();
        };
        let mut t = Self::new(world.mins, world.maxs, MAX_ENTITIES);
        for (lo, hi) in vcod_common::props::area_bounds(fs, &bsp.entities) {
            t.link_static(lo.to_array(), hi.to_array());
        }
        t
    }

    /// What an entity's last link handed the tree, `None` while it is not
    /// linked: a game function that relinks in place starts from it.
    pub fn last_link(&self, ent: u32) -> Option<Link> {
        let e = self.ents.get(ent as usize).filter(|e| e.node.is_some())?;
        Some(Link {
            absmin: e.absmin,
            absmax: e.absmax,
            contents: e.contents,
            tree: Some((e.stored.mins, e.stored.maxs)),
        })
    }

    /// `G_ShutdownGame`'s free of every entity in use, which a
    /// `map_restart` runs before the new level spawns into the same tree:
    /// the entities leave, the nodes the static models hold stay.
    pub fn unlink_all(&mut self) {
        for n in 0..self.ents.len() as u32 {
            self.unlink(n);
        }
    }

    /// The contents an entity last linked with, 0 when it is not linked.
    pub fn contents(&self, ent: u32) -> i32 {
        self.ents
            .get(ent as usize)
            .filter(|e| e.node.is_some())
            .map_or(0, |e| e.contents)
    }

    /// A write of `r.contents` with no link behind it (`SP_trigger_hurt`,
    /// `trigger_use`, `notSolid` on a brush model): the entity keeps its
    /// place and the masks above it, and the walk tests the new contents.
    pub fn set_contents(&mut self, ent: u32, contents: i32) {
        if let Some(e) = self.ents.get_mut(ent as usize) {
            e.contents = contents;
        }
    }

    /// `0x80594c4`: a static model, filed once at map load in the order the
    /// entity string lists them. It never moves, but it keeps every node on
    /// its path from being freed and its push-downs split nodes the entities
    /// later share.
    pub fn link_static(&mut self, mins: [f32; 3], maxs: [f32; 3]) {
        let b = Bounds2 {
            mins: [mins[0], mins[1]],
            maxs: [maxs[0], maxs[1]],
        };
        let mut region = self.world;
        let node = self.descend(&b, &mut region);
        let id = self.statics.len() as u32;
        self.statics.push(StaticLink {
            node,
            next: self.nodes[node as usize].statics,
            bounds: b,
        });
        self.nodes[node as usize].statics = Some(id);
        self.push_down(node, region);
    }

    /// `SV_LinkEntity`'s tail (`0x8059344`), or the unlink it takes for
    /// contents 0. An entity that lands in the node it already sits in with
    /// no contents bit dropped keeps its place in the list; anything else is
    /// unlinked and prepended to the node it lands in.
    pub fn link(&mut self, ent: u32, link: &Link) {
        let e = ent as usize;
        if e >= self.ents.len() {
            return;
        }
        self.ents[e].absmin = link.absmin;
        self.ents[e].absmax = link.absmax;
        self.ents[e].contents = link.contents;
        if link.contents == 0 {
            self.unlink(ent);
            return;
        }
        let b = link.tree_box();
        loop {
            let old = self.ents[e];
            let mut region = self.world;
            let keeps =
                |node: NodeId| old.node == Some(node) && old.stored_contents & !link.contents == 0;
            let (node, straddles) = self.descend_ent(&b, &mut region, link.contents);
            if straddles && keeps(node) {
                self.ents[e].stored_contents = link.contents;
                self.ents[e].stored = b;
                return;
            }
            if old.node.is_none() {
                self.ents[e].node = Some(node);
                self.ents[e].next = self.nodes[node as usize].ents;
                self.nodes[node as usize].ents = Some(ent);
            } else if !keeps(node) {
                self.unlink(ent);
                continue;
            }
            self.ents[e].stored_contents = link.contents;
            self.ents[e].stored = b;
            self.push_down(node, region);
            return;
        }
    }

    /// `0x8058e9c`: out of its node's list, every emptied node on the way up
    /// back to the pool, and the masks above recomputed.
    pub fn unlink(&mut self, ent: u32) {
        let e = ent as usize;
        let Some(node) = self.ents.get(e).and_then(|l| l.node) else {
            return;
        };
        let next = self.ents[e].next;
        self.ents[e].node = None;
        self.ents[e].next = None;
        if self.nodes[node as usize].ents == Some(ent) {
            self.nodes[node as usize].ents = next;
        } else {
            let mut at = self.nodes[node as usize].ents;
            while let Some(i) = at {
                if self.ents[i as usize].next == Some(ent) {
                    self.ents[i as usize].next = next;
                    break;
                }
                at = self.ents[i as usize].next;
            }
        }
        let mut n = node;
        loop {
            let nd = self.nodes[n as usize];
            if nd.ents.is_some() || nd.statics.is_some() || nd.above.is_some() || nd.below.is_some()
            {
                break;
            }
            self.nodes[n as usize].mask = 0;
            let Some(parent) = nd.parent else { break };
            self.free.push(n);
            let p = &mut self.nodes[parent as usize];
            if p.above == Some(n) {
                p.above = None;
            } else {
                p.below = None;
            }
            n = parent;
        }
        let mut at = Some(n);
        while let Some(i) = at {
            let nd = self.nodes[i as usize];
            let mut mask = nd.above.map_or(0, |c| self.nodes[c as usize].mask)
                | nd.below.map_or(0, |c| self.nodes[c as usize].mask);
            let mut e = nd.ents;
            while let Some(x) = e {
                mask |= self.ents[x as usize].contents;
                e = self.ents[x as usize].next;
            }
            self.nodes[i as usize].mask = mask;
            at = nd.parent;
        }
    }

    /// `SV_AreaEntities` (`0x805a540` through `0x8059590`): a node's own
    /// list from its head, then the side above its split, then the side
    /// below, each entity whose contents meet `mask` and whose world box
    /// touches `mins`..`maxs`.
    pub fn entities_in_box(&self, mins: [f32; 3], maxs: [f32; 3], mask: i32) -> Vec<u32> {
        let mut out = Vec::new();
        self.query(ROOT, &mins, &maxs, mask, &mut out);
        out
    }

    fn query(&self, n: NodeId, mins: &[f32; 3], maxs: &[f32; 3], mask: i32, out: &mut Vec<u32>) {
        let nd = &self.nodes[n as usize];
        if nd.mask & mask == 0 {
            return;
        }
        let mut at = nd.ents;
        while let Some(i) = at {
            let e = &self.ents[i as usize];
            let touches = (0..3).all(|k| maxs[k] >= e.absmin[k] && mins[k] <= e.absmax[k]);
            if e.contents & mask != 0 && touches {
                out.push(i);
            }
            at = e.next;
        }
        if let Some(c) = nd.above
            && nd.dist < maxs[nd.axis]
        {
            self.query(c, mins, maxs, mask, out);
        }
        if let Some(c) = nd.below
            && mins[nd.axis] < nd.dist
        {
            self.query(c, mins, maxs, mask, out);
        }
    }

    /// The walk down from the root for an entity, OR-ing its contents into
    /// every node it passes: the node it stops at, the region that node
    /// covers, and whether it stopped because the box touches the split
    /// rather than for want of a child.
    fn descend_ent(&mut self, b: &Bounds2, region: &mut Bounds2, contents: i32) -> (NodeId, bool) {
        let mut n = ROOT;
        loop {
            self.nodes[n as usize].mask |= contents;
            let nd = self.nodes[n as usize];
            let next = if nd.dist < b.mins[nd.axis] {
                region.mins[nd.axis] = nd.dist;
                nd.above
            } else if nd.dist <= b.maxs[nd.axis] {
                return (n, true);
            } else {
                region.maxs[nd.axis] = nd.dist;
                nd.below
            };
            match next {
                Some(c) => n = c,
                None => return (n, false),
            }
        }
    }

    /// [`Self::descend_ent`] for a static model, which carries no mask the
    /// entity query reads.
    fn descend(&self, b: &Bounds2, region: &mut Bounds2) -> NodeId {
        let mut n = ROOT;
        loop {
            let nd = self.nodes[n as usize];
            let next = if nd.dist < b.mins[nd.axis] {
                region.mins[nd.axis] = nd.dist;
                nd.above
            } else if nd.dist <= b.maxs[nd.axis] {
                return n;
            } else {
                region.maxs[nd.axis] = nd.dist;
                nd.below
            };
            match next {
                Some(c) => n = c,
                None => return n,
            }
        }
    }

    /// `0x8058f80`: one pass over `n`'s entity list, then its static list,
    /// moving every entry that lies wholly on one side of the split into
    /// that child, prepended, and creating the child when it is missing.
    /// The child is cut on `region`, `n`'s own region, not the child's half
    /// of it. A child that cannot be made (the region is 512 or less, or
    /// the pool is empty) ends the whole pass.
    fn push_down(&mut self, n: NodeId, region: Bounds2) {
        let (axis, dist) = (self.nodes[n as usize].axis, self.nodes[n as usize].dist);
        let mut prev: Option<u32> = None;
        let mut at = self.nodes[n as usize].ents;
        while let Some(i) = at {
            let e = self.ents[i as usize];
            let above = dist < e.stored.mins[axis];
            if !above && e.stored.maxs[axis] >= dist {
                prev = Some(i);
                at = e.next;
                continue;
            }
            let Some(child) = self.child(n, above, &region) else {
                return;
            };
            at = e.next;
            self.ents[i as usize].node = Some(child);
            self.ents[i as usize].next = self.nodes[child as usize].ents;
            self.nodes[child as usize].ents = Some(i);
            self.nodes[child as usize].mask |= e.contents;
            match prev {
                None => self.nodes[n as usize].ents = at,
                Some(p) => self.ents[p as usize].next = at,
            }
        }
        let mut prev: Option<u32> = None;
        let mut at = self.nodes[n as usize].statics;
        while let Some(i) = at {
            let s = self.statics[i as usize];
            let above = dist < s.bounds.mins[axis];
            if !above && s.bounds.maxs[axis] >= dist {
                prev = Some(i);
                at = s.next;
                continue;
            }
            let Some(child) = self.child(n, above, &region) else {
                return;
            };
            at = s.next;
            self.statics[i as usize].node = child;
            self.statics[i as usize].next = self.nodes[child as usize].statics;
            self.nodes[child as usize].statics = Some(i);
            match prev {
                None => self.nodes[n as usize].statics = at,
                Some(p) => self.statics[p as usize].next = at,
            }
        }
    }

    /// `n`'s child on one side, made from the pool when missing.
    fn child(&mut self, n: NodeId, above: bool, region: &Bounds2) -> Option<NodeId> {
        let nd = self.nodes[n as usize];
        if let Some(c) = if above { nd.above } else { nd.below } {
            return Some(c);
        }
        // The pool is tested before the size.
        if self.free.is_empty() {
            return None;
        }
        let axis = split_axis(region);
        if region.maxs[axis] - region.mins[axis] <= MIN_SPLIT {
            return None;
        }
        let c = self.free.pop()?;
        self.nodes[c as usize] = Node {
            axis,
            dist: (region.maxs[axis] + region.mins[axis]) * 0.5,
            parent: Some(n),
            ..Node::default()
        };
        if above {
            self.nodes[n as usize].above = Some(c);
        } else {
            self.nodes[n as usize].below = Some(c);
        }
        Some(c)
    }
}

#[cfg(test)]
impl AreaTree {
    fn nodes_in_use(&self) -> usize {
        POOL + 1 - self.free.len()
    }
}

/// y when the region is at least as deep in y as it is wide in x.
fn split_axis(r: &Bounds2) -> usize {
    usize::from(r.maxs[0] - r.mins[0] <= r.maxs[1] - r.mins[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: i32 = 0x0200_0000;
    const CORPSE: i32 = 0x0400_0000;

    fn player(x: f32, y: f32) -> Link {
        Link::player([x, y, 0.0], [-15.0, -15.0, 0.0], [15.0, 15.0, 72.0], BODY)
    }

    fn everything(t: &AreaTree) -> Vec<u32> {
        t.entities_in_box([-1e5; 3], [1e5; 3], -1)
    }

    /// A 4096-square world: the root splits y at 0 (a tie goes to y).
    fn square() -> AreaTree {
        AreaTree::new([-2048.0, -2048.0, -64.0], [2048.0, 2048.0, 64.0], 64)
    }

    #[test]
    fn a_list_is_most_recently_linked_first_and_a_relink_in_place_keeps_its_slot() {
        let mut t = square();
        // All three straddle the root's y = 0 split, so all sit in the root.
        for (n, x) in [(0, -300.0), (1, 0.0), (2, 300.0)] {
            t.link(n, &player(x, 10.0));
        }
        assert_eq!(everything(&t), [2, 1, 0]);
        // Moved along but still across the split: same node, same contents.
        t.link(0, &player(-280.0, -20.0));
        assert_eq!(everything(&t), [2, 1, 0]);
        // An unlink first, as `setOrigin` on a player does, goes to the head.
        t.unlink(0);
        t.link(0, &player(-280.0, -20.0));
        assert_eq!(everything(&t), [0, 2, 1]);
        // A contents bit dropped (`player_die`'s corpse) relinks at the head.
        t.link(
            1,
            &Link {
                contents: CORPSE,
                ..player(0.0, 10.0)
            },
        );
        assert_eq!(everything(&t), [1, 0, 2]);
        // A bit added keeps the slot.
        t.link(
            2,
            &Link {
                contents: BODY | 1,
                ..player(300.0, 10.0)
            },
        );
        assert_eq!(everything(&t), [1, 0, 2]);
    }

    #[test]
    fn a_walk_takes_a_node_then_the_side_above_then_the_side_below() {
        let mut t = square();
        t.link(0, &player(0.0, -500.0));
        t.link(1, &player(0.0, 500.0));
        t.link(2, &player(0.0, 0.0));
        assert_eq!(everything(&t), [2, 1, 0]);
        // A box wholly below the split never reaches the side above.
        let below = t.entities_in_box([-100.0, -600.0, -10.0], [100.0, -400.0, 10.0], -1);
        assert_eq!(below, [0]);
        // Contents outside the mask are skipped.
        assert!(t.entities_in_box([-1e5; 3], [1e5; 3], CORPSE).is_empty());
    }

    #[test]
    fn a_push_down_goes_one_level_and_reverses_what_it_moves() {
        let mut t = square();
        // 0 stops at the root for want of a child below; the push-down cuts
        // the lower half (x at 0) and moves 0 into it.
        t.link(0, &player(-500.0, -500.0));
        assert_eq!(t.nodes_in_use(), 2);
        // 1 stops at that child, below its split with nothing there, and is
        // prepended: [1, 0]. The push-down cuts the next level and moves
        // both, from the head, each prepended there: [0, 1].
        t.link(1, &player(-700.0, -500.0));
        assert_eq!(t.nodes_in_use(), 3);
        assert_eq!(everything(&t), [0, 1]);
    }

    #[test]
    fn an_emptied_node_goes_back_to_the_pool_and_a_small_region_never_splits() {
        let mut t = square();
        t.link(0, &player(-500.0, -500.0));
        let used = t.nodes_in_use();
        assert!(used > 1);
        t.unlink(0);
        assert_eq!(t.nodes_in_use(), 1, "the chain under the root is freed");
        // 600 units square: the root splits once at x 0 and its 300-wide
        // halves never split.
        let mut t = AreaTree::new([-300.0, -299.0, 0.0], [300.0, 299.0, 0.0], 8);
        for n in 0..4 {
            t.link(n, &player(-200.0 + n as f32, 0.0));
        }
        assert!(t.nodes_in_use() <= 2);
    }

    /// `probe_blastorder` on retail (combat doc 14.7): four players set down
    /// round mp_carentan's second split, then moved one at a time, each
    /// `setOrigin` an unlink and a link. The static models are linked first,
    /// as the map load does; they build the nodes the players land in.
    #[test]
    fn the_tree_replays_probe_blastorder_on_mp_carentan() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = fs.read("maps/mp/mp_carentan.bsp").unwrap();
        let bsp = vcod_common::bsp::parse(&bytes).unwrap();
        let (mins, maxs) = bsp.visibility().model_bounds(0).unwrap();
        let mut t = AreaTree::new(mins, maxs, 1024);
        for (lo, hi) in vcod_common::props::area_bounds(&fs, &bsp.entities) {
            t.link_static(lo.to_array(), hi.to_array());
        }
        let blast = |t: &AreaTree| {
            let r = 500.0 * std::f32::consts::SQRT_2;
            let o = [-176.8f32, 2473.1, 7.0];
            t.entities_in_box(o.map(|v| v - r), o.map(|v| v + r), -1)
        };
        let set = |t: &mut AreaTree, n: u32, x: f32, y: f32| {
            t.unlink(n);
            t.link(n, &player(x, y));
        };
        set(&mut t, 0, -290.0, 2430.0);
        set(&mut t, 1, -260.0, 2480.0);
        set(&mut t, 2, -230.0, 2380.0);
        set(&mut t, 3, -230.0, 2540.0);
        assert_eq!(blast(&t), [1, 0, 3, 2], "placed");
        set(&mut t, 0, -290.0, 2440.0);
        assert_eq!(blast(&t), [0, 1, 3, 2], "moved_within");
        set(&mut t, 2, -200.0, 2430.0);
        assert_eq!(blast(&t), [2, 0, 1, 3], "moved_into");
        set(&mut t, 1, -290.0, 2380.0);
        assert_eq!(blast(&t), [2, 0, 3, 1], "moved_out");
    }
}
