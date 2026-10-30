//! CoD2 bots fetching a dropped gun (docs/bots.md, "CoD2"). Retail has no
//! bots and the CoD1 brain fetches only in UO's `bas`, so the rule is
//! vcod's own: a bot whose primaries are dry, with no enemy in sight,
//! walks to the nearest loaded gun lying within [`REACH`] on a clear
//! walk, the dry primary in hand so the swap drops it, and taps use once
//! the cursor hint names the gun (`Player_UpdateActivate`'s swap,
//! `game::cod2_use`). A gun not taken in [`GIVE_UP_MS`] is passed over
//! for [`SHUN_MS`].

use glam::Vec3;
use vcod_cod2::net::UserCmd2;
use vcod_gsc::EntId;

use super::{CLASS_PISTOL, Cod2Bots, TYPE_BULLET, playing};
use crate::bots::BotView;
use crate::game::cod2::Cod2Level;

/// How far off a dropped gun is fetched, and how far up or down.
const REACH: f32 = 600.0;
const STEP: f32 = 40.0;
/// Within this the view turns down onto the gun for the press.
const LOOK: f32 = 120.0;
/// A fetch gives up after this, ms, and the gun is passed over this long.
const GIVE_UP_MS: i32 = 6000;
const SHUN_MS: i32 = 20_000;
/// `CONTENTS_USE`: what an item links with (`game::cod2_use`).
const CONTENTS_USE: i32 = 0x20_0000;
/// The walk's box, the step height up, as the gun walk's.
const WALK_MINS: Vec3 = Vec3::new(-15.0, -15.0, 18.0);
const WALK_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// One bot's fetch: the gun, when it set off, the last cmd's use, and
/// the guns it passed over with when each is fair game again.
#[derive(Clone, Debug, Default)]
pub(super) struct Fetch {
    item: Option<(EntId, Vec3)>,
    since: i32,
    pressed: bool,
    shunned: Vec<(EntId, i32)>,
}

/// What the fetch makes of the brain's cmd near the gun.
pub(super) struct FetchCmd {
    /// The view onto the gun, once within [`LOOK`].
    look: Option<[f32; 2]>,
    /// The dry primary to hold for the swap.
    weapon: Option<u8>,
    press: bool,
}

impl Cod2Bots {
    /// The fetch step for the bot in `slot` this tick, after its route: a
    /// walk to a gun sets `view`'s waypoint.
    pub(super) fn fetch_step(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        view: &mut BotView,
    ) -> Option<FetchCmd> {
        let now = level.level_time();
        let p = level.players.get(slot)?.as_ref()?;
        let mut task = self.fetches.remove(&slot).unwrap_or_default();
        task.shunned.retain(|s| now.wrapping_sub(s.1) < SHUN_MS);
        let defs = &level.combat.weapons;
        let def = |w: u8| defs.get(usize::from(w)).and_then(Option::as_ref);
        let primary = |w: u8| {
            def(w).is_some_and(|d| {
                d.weap_type == TYPE_BULLET && d.weap_class != CLASS_PISTOL && d.offhand_class == 0
            })
        };
        // A primary carried with no round in it.
        let dry = (1u8..64).find(|&w| view.weapons_held >> w & 1 == 1 && primary(w));
        let wants = playing(level, slot)
            && view.gun.is_none()
            && view.enemy.is_none()
            && view.on_ground
            && !view.linked
            && self.shoot;
        if !wants {
            if !playing(level, slot) {
                task.item = None;
            } else if task.item.is_some() {
                // A fight or a gun come to hand ends the walk.
                task.item = None;
            }
            self.fetches.insert(slot, task);
            return None;
        }
        let lying = weapon_items(level, Vec3::from(view.origin), &|w| primary(w));
        if let Some((id, _)) = task.item
            && (now.wrapping_sub(task.since) > GIVE_UP_MS || !lying.iter().any(|l| l.0 == id))
        {
            if lying.iter().any(|l| l.0 == id) {
                task.shunned.push((id, now));
            }
            task.item = None;
        }
        if task.item.is_none() {
            let feet = Vec3::from(view.origin);
            let w = &level.world.collision;
            let taken: Vec<EntId> = (self.fetches.values())
                .filter_map(|t| t.item.map(|i| i.0))
                .collect();
            task.item = lying
                .iter()
                .filter(|(id, _)| !taken.contains(id) && !task.shunned.iter().any(|s| s.0 == *id))
                .filter(|(_, at)| (at.z - feet.z).abs() < STEP + 24.0)
                .filter(|(_, at)| {
                    let to = Vec3::new(at.x, at.y, feet.z);
                    w.move_trace(
                        feet,
                        to,
                        WALK_MINS,
                        WALK_MAXS,
                        vcod_cod2::pmove::MASK_PLAYERSOLID,
                    )
                    .fraction
                        >= 1.0
                })
                .min_by(|a, b| a.1.distance(feet).total_cmp(&b.1.distance(feet)))
                .copied();
            task.since = now;
            task.pressed = false;
        }
        let Some((id, at)) = task.item else {
            self.fetches.insert(slot, task);
            return None;
        };
        view.waypoint = Some(at.to_array());
        view.stop = false;
        let eye = super::eye(&p.ps);
        let near = (at - eye).truncate().length() < LOOK;
        let look = near.then(|| {
            let d = at - eye;
            // The view's pitch stops at 80: a gun underfoot settles there.
            [
                (-d.z.atan2(d.truncate().length()).to_degrees()).clamp(-80.0, 80.0),
                d.y.atan2(d.x).to_degrees(),
            ]
        });
        // The swap drops the gun in hand: the dry primary, not the pistol.
        let weapon = dry.filter(|_| near);
        let ready = weapon.is_none_or(|w| p.ws.weapon == w);
        let press = ready && p.gunner.hint_ent == Some(id) && !task.pressed;
        task.pressed = press;
        self.fetches.insert(slot, task);
        Some(FetchCmd {
            look,
            weapon,
            press,
        })
    }

    /// Whether the bot in `slot` is on its way to a gun.
    pub(super) fn fetching(&self, slot: usize) -> bool {
        self.fetches.get(&slot).is_some_and(|t| t.item.is_some())
    }
}

/// The fetch's cmd over the brain's: the view turned onto the gun at the
/// bot's rate with the keys following, the dry primary held, the trigger
/// and sight up, and the use press.
pub(super) fn apply(c: &mut UserCmd2, f: &FetchCmd, view: &BotView, turn_deg: f32) {
    if let Some(want) = f.look {
        let step = |from: f32, to: f32| {
            let d = (to - from + 180.0).rem_euclid(360.0) - 180.0;
            from + d.clamp(-turn_deg, turn_deg)
        };
        let look = [step(view.view[0], want[0]), step(view.view[1], want[1])];
        let from = (c.angles[1] + view.delta_angles[1]) as f32 * 360.0 / 65536.0;
        (c.forward_move, c.right_move) =
            crate::bots::rekey(c.forward_move, c.right_move, from, look[1]);
        c.angles = crate::bots::cmd_angles(view, look);
    }
    if let Some(w) = f.weapon {
        c.weapon = w;
    }
    c.buttons &= !(vcod_cod2::pmove::button::ATTACK | vcod_cod2::pmove::button::ADS);
    if f.press {
        c.buttons |= super::gun::USE;
    }
}

/// The guns lying within [`REACH`] of `at` that `wanted` takes, by item
/// entity and bounds centre: not taken, a weapon row, loaded (`clip` not
/// -1).
fn weapon_items(level: &Cod2Level, at: Vec3, wanted: &dyn Fn(u8) -> bool) -> Vec<(EntId, Vec3)> {
    use crate::game::pickup::{ItemKind, item_kind};
    let host = &level.rt.host;
    let r = Vec3::new(REACH, REACH, 128.0);
    let mut out = Vec::new();
    for n in host
        .area
        .entities_in_box((at - r).to_array(), (at + r).to_array(), CONTENTS_USE)
    {
        let (Some(id), Some(link)) = (host.ents.handle(n), host.area.last_link(n)) else {
            continue;
        };
        let Some(item) = host.ents.get(id).and_then(|e| e.item) else {
            continue;
        };
        if item.taken || item.clip < 0 {
            continue;
        }
        let Some(ItemKind::Weapon(w)) = item_kind(host.defs, usize::from(item.index)) else {
            continue;
        };
        if !wanted(w) {
            continue;
        }
        let centre = (Vec3::from(link.absmin) + Vec3::from(link.absmax)) * 0.5;
        if centre.distance(at) <= REACH {
            out.push((id, centre));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// A lone bot whose primary has run dry walks to a gun dropped near it
    /// and takes it with the use key. Needs `COD2_DIR`.
    #[test]
    fn a_dry_bot_fetches_a_dropped_gun() {
        use glam::Vec3;
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut sv = crate::cod2::Cod2Server::new(
            crate::cod2::Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "bots".into(),
                max_clients: 8,
                gametype_script: None,
            },
            3,
            now,
        );
        sv.load(Some(std::rc::Rc::new(fs))).unwrap();
        let slot = sv.add_bots(1, true)[0];
        sv.bots_mut().unwrap().wait_for_graph();
        let at = |t: u64| now + std::time::Duration::from_millis(50 * t);
        let mut tick = 0;
        while tick < 200 && !sv.level().unwrap().playing(slot) {
            tick += 1;
            sv.tick_frames(at(tick), 1);
        }
        for _ in 0..20 {
            tick += 1;
            sv.tick_frames(at(tick), 1);
        }
        let level = sv.level_mut().unwrap();
        let p = level.players[slot].as_ref().unwrap();
        let (origin, held) = (p.ps.origin, p.ws.held);
        let defs = level.combat.weapons.clone();
        let primary = |w: usize| {
            defs.get(w)
                .and_then(Option::as_ref)
                .is_some_and(|d| d.weap_type == 0 && matches!(d.weap_class, 0 | 2))
        };
        let mine = (1..64).find(|&w| held >> w & 1 == 1 && primary(w)).unwrap();
        let other = (1..64).find(|&w| held >> w & 1 == 0 && primary(w)).unwrap();
        // A spot 150 units off along a clear walk.
        let w = &level.world.collision;
        let (lo, hi) = (Vec3::new(-15.0, -15.0, 18.0), Vec3::new(15.0, 15.0, 70.0));
        let drop = (0..8)
            .map(|i| {
                let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
                origin + Vec3::new(c, s, 0.0) * 150.0
            })
            .find(|&to| w.move_trace(origin, to, lo, hi, 1).fraction >= 1.0)
            .unwrap();
        let d = defs[mine].as_ref().unwrap();
        let (ci, ai) = (d.clip_index, d.ammo_index);
        level.rt.with_host(|host, cx| {
            host.client_ammo[slot].clip[ci] = 0;
            host.client_ammo[slot].ammo[ai] = 0;
            crate::game::item::launch(host, cx, other, drop + Vec3::Z * 16.0, Vec3::ZERO, None)
                .unwrap();
        });
        let mut fetched = false;
        for _ in 0..200 {
            tick += 1;
            sv.tick_frames(at(tick), 1);
            fetched |= sv.bots_mut().unwrap().fetching(slot);
            let p = sv.level().unwrap().players[slot].as_ref().unwrap();
            if p.ws.held >> other & 1 == 1 {
                assert!(fetched);
                return;
            }
        }
        panic!("never took weapon {other} (fetching {fetched})");
    }
}
