//! `G_FindTeams` (1.1d 0x4f9c4, 1.5b 0x4e88a, UO 0x57e4f): the `teamname`
//! chains the map's entities form at the load, and the item pick that walks
//! them (cod11-server-handshake.md, "Console commands over rcon";
//! cod11-items.md 14.8).

use crate::game::host::GameHost;
use vcod_gsc::{Cx, EntId, Host, Value};

/// `teamchain` (`+0x260`), `teammaster` (`+0x264`) and the slave bit
/// (`flags & 4`) of one entity, by entity number. A freed slot's are
/// cleared with it, as `G_FreeEntity`'s memset clears retail's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TeamLinks {
    pub chain: Option<u32>,
    pub master: Option<u32>,
    pub slave: bool,
}

/// The classname `G_FindTeams` compares against `scr_const+0x3a`.
const TRAMCAR: &str = "func_tramcar";

/// `G_FindTeams`, right after the spawn pass: over entities 1 up to
/// `level.num_entities`, one in use with a `teamname` and no slave bit is a
/// master, unless it is a `func_tramcar` without spawnflags 8; only a
/// tramcar master is its own `teammaster`. Every later entity with the same
/// string (case-sensitive) goes in behind the master, so a chain runs
/// master, last, ..., first; a tramcar slave is unlinked. Returns the
/// masters and the masters plus slaves, the banner's two counts.
pub fn find_teams(host: &mut GameHost, cx: &mut Cx) -> (u32, u32) {
    let team_atom = cx.intern_folded("teamname");
    let class_atom = cx.intern_folded("classname");
    let flags_atom = cx.intern_folded("spawnflags");
    // Each live entity's team string, whether it is a tramcar, its
    // spawnflags: what the two loops read.
    let n = host.ents.num_entities();
    let mut rows: Vec<Option<(vcod_gsc::Atom, bool, i32)>> = vec![None; n as usize];
    for (num, row) in rows.iter_mut().enumerate().skip(1) {
        let Some(id) = host.ents.handle(num as u32) else {
            continue;
        };
        let Value::String(team) = host.get_field(cx, id, team_atom) else {
            continue;
        };
        let tramcar = matches!(host.get_field(cx, id, class_atom),
            Value::String(c) if cx.resolve(c) == TRAMCAR);
        let flags = match host.get_field(cx, id, flags_atom) {
            Value::Int(f) => f,
            _ => 0,
        };
        *row = Some((team, tramcar, flags));
    }
    let (mut teams, mut members) = (0, 0);
    for i in 1..n {
        let Some((team, tramcar, flags)) = rows[i as usize] else {
            continue;
        };
        if is_slave(host, i) || (tramcar && flags & 8 == 0) {
            continue;
        }
        if tramcar {
            set_links(host, i, |l| l.master = Some(i));
        }
        teams += 1;
        members += 1;
        for j in i + 1..n {
            let Some((other, slave_tramcar, _)) = rows[j as usize] else {
                continue;
            };
            if other != team || is_slave(host, j) {
                continue;
            }
            members += 1;
            let head = links(host, i).chain;
            set_links(host, j, |l| {
                l.chain = head;
                l.master = Some(i);
                l.slave = true;
            });
            set_links(host, i, |l| l.chain = Some(j));
            if slave_tramcar && let Some(id) = host.ents.handle(j) {
                unlink(host, id);
            }
        }
    }
    host.team_counts = (teams, members);
    (teams, members)
}

fn links(host: &GameHost, n: u32) -> TeamLinks {
    host.ents
        .handle(n)
        .and_then(|id| host.ents.get(id))
        .map_or_else(TeamLinks::default, |e| e.team)
}

fn is_slave(host: &GameHost, n: u32) -> bool {
    links(host, n).slave
}

fn set_links(host: &mut GameHost, n: u32, f: impl FnOnce(&mut TeamLinks)) {
    if let Some(id) = host.ents.handle(n)
        && let Some(e) = host.ents.get_mut_quiet(id)
    {
        f(&mut e.team);
    }
}

/// `trap_UnlinkEntity` on a tramcar slave (0x4facd): out of the area tree
/// and, with a `*N`, its brushes out of the traces.
fn unlink(host: &mut GameHost, id: EntId) {
    host.area.unlink(id.0);
    let model = host.ents.get(id).and_then(|e| e.brush_model);
    if let (Some(n), Some(world)) = (model, &host.world) {
        world.collision.set_model_linked(n as usize, false);
    }
}

/// `G_RunMover`'s slave arm (1.1d 0x57630..0x57664): an `eType` 5 or 8
/// entity (`InitMover`'s, or a `script_*` one's, `InitScriptMover`) that
/// is a team slave runs neither `G_MoverTeam` nor `G_RunThink`, so a
/// `delete()`'s free never comes (cod11-movers.md 25). The arm's unlink of
/// a linked `func_tramcar` or `func_rotating` slave is not modelled: vcod
/// builds neither as a mover.
pub fn idle_slave(host: &mut GameHost, cx: &mut Cx, id: EntId) -> bool {
    if !host.ents.get(id).is_some_and(|e| e.team.slave) {
        return false;
    }
    if crate::game::mover::is_mover(host, id) {
        return true;
    }
    let class = cx.intern_folded("classname");
    matches!(host.get_field(cx, id, class), Value::String(c)
        if matches!(cx.resolve(c), "script_brushmodel" | "script_model" | "script_origin"))
}

/// What `RespawnItem` (1.1d 0x4ec7c) and `Use_Item` (0x4efc8; 1.5b and UO
/// call `RespawnItem`) respawn in place of `id`.
pub enum Pick {
    /// The entity the body runs on: `id` itself without a `teamname`, else
    /// the member `rand() % count` links down the chain from its
    /// `teammaster`.
    Ent(EntId),
    /// The pick landed on a slot freed since the load, which the chain
    /// still counts (its links cleared): retail runs the body on the freed
    /// slot, vcod on nothing.
    Freed,
    /// A `teamname` and a null `teammaster`, every master but a
    /// tramcar's: `G_Error("RespawnItem: bad teammaster")` (0x74bf4).
    BadTeammaster,
}

/// `G_Error`'s text for [`Pick::BadTeammaster`].
pub const BAD_TEAMMASTER: &str = "RespawnItem: bad teammaster";

/// The team branch of `RespawnItem`: the count from `teammaster` down
/// `teamchain`, one draw (a pinned one first, `Server::pin_item_draws`),
/// and that many links.
pub fn respawn_pick(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Pick {
    let team = cx.intern_folded("teamname");
    if !matches!(host.get_field(cx, id, team), Value::String(_)) {
        return Pick::Ent(id);
    }
    let Some(master) = host.ents.get(id).and_then(|e| e.team.master) else {
        return Pick::BadTeammaster;
    };
    let walk = |host: &GameHost, steps: Option<i32>| {
        let (mut at, mut count) = (Some(master), 0);
        while let Some(n) = at {
            if steps == Some(count) {
                return (at, count);
            }
            at = links(host, n).chain;
            count += 1;
        }
        (None, count)
    };
    let (_, count) = walk(host, None);
    let draw = match host.item_draw_pins.pop_front() {
        Some(r) => r,
        None => host.rand_int(),
    };
    let (at, _) = walk(host, Some(draw % count));
    match at.and_then(|n| host.ents.handle(n)) {
        Some(e) => Pick::Ent(e),
        None => Pick::Freed,
    }
}
