//! The quads over the world view: the crosshair, overhead names, the low
//! health overlay, damage direction icons and offscreen waypoints.

use vcod_cod4::hud::{self as h, HudElem};

use super::assets::Loaded;
use super::compass::ENTITYNUM_NONE;
use super::elems::{colour, material_size};
use super::weapon_info::{LOW_HEALTH_MENU, OD_LOW_HEALTH};
use super::{Cod4Hud, PM_SPECTATOR, image_of, quad};
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::play::cod4::{Cod4Session, client_name};

impl Cod4Hud {
    /// Weapon `weapon`'s `enemyCrosshairRange` off the zones' defs; 0
    /// before they load or for no weapon.
    pub(crate) fn enemy_crosshair_range(&self, s: &Cod4Session, weapon: u32) -> f32 {
        let (Some(Some(l)), Some(gs)) = (&self.loaded, s.gamestate()) else {
            return 0.0;
        };
        let list = crate::play::cod4::weapon_list(gs);
        (weapon as usize)
            .checked_sub(1)
            .and_then(|i| list.get(i))
            .and_then(|n| l.weapon_hud.get(n))
            .map_or(0.0, |d| d.reticle.enemy_range)
    }

    /// The crosshair and a sight's overlay (`cod4_crosshair`) off the
    /// drawn playerstate `ps` at a fov whose half tangents are `tan`
    /// (x, y); a gunner's off the turret he is locked to; nothing until
    /// the zones load, for a dead body or a spectator, or under a script
    /// menu (cod4-hud.md 17).
    pub(crate) fn crosshair(
        &self,
        s: &Cod4Session,
        ps: &crate::cod4_viewmodel::ViewPs,
        tan: [f32; 2],
        size: (f32, f32),
        shell: &Shell,
    ) -> Vec<HudQuad> {
        let Some(Some(l)) = &self.loaded else {
            return Vec::new();
        };
        if ps.pm_type >= PM_SPECTATOR || s.menus.open().is_some() {
            return Vec::new();
        }
        let Some(gs) = s.gamestate() else {
            return Vec::new();
        };
        let list = crate::play::cod4::weapon_list(gs);
        let def_of = |w: u32| {
            (w as usize)
                .checked_sub(1)
                .and_then(|i| list.get(i))
                .and_then(|n| l.weapon_hud.get(n))
        };
        // `CG_PlayerTurretWeaponIdx`: the locked turret entity's weapon.
        let turret = s
            .client()
            .snapshots
            .last()
            .filter(|_| ps.viewlocked != 0 && ps.viewlocked_ent != ENTITYNUM_NONE)
            .and_then(|snap| {
                let e = snap
                    .entities
                    .iter()
                    .find(|e| e.number() == ps.viewlocked_ent)?;
                def_of(crate::play::cod4::ent_u32(e, "weapon"))
            });
        static NO_WEAPON: std::sync::OnceLock<vcod_cod4::hud::WeaponHud> =
            std::sync::OnceLock::new();
        let Some(def) = def_of(ps.view_weapon())
            .or_else(|| turret.map(|_| NO_WEAPON.get_or_init(Default::default)))
        else {
            return Vec::new();
        };
        let rgb = |n: &str| self.team_rgb(s, n);
        let view = crate::cod4_crosshair::View {
            ps,
            def,
            tan_half_fov_y: tan[1],
            tan_half_fov_x: tan[0],
            turret,
            to_ads: ps.pm_flags & crate::cod4_viewmodel::PMF_SIGHT != 0,
            team_rgb: [rgb("g_TeamColor_MyTeam"), rgb("g_TeamColor_EnemyTeam")],
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        crate::cod4_crosshair::quads(
            &view,
            &crate::cod4_crosshair::Dvars::read(shell),
            &sp,
            &|m| image_of(l, m),
        )
    }

    /// A `g_TeamColor_*` server dvar's colour, white where unset.
    pub(crate) fn team_rgb(&self, s: &Cod4Session, name: &str) -> [f32; 3] {
        let v = self.server_dvar(s, name).unwrap_or_default();
        let mut c = v
            .split_whitespace()
            .map(|x| x.parse::<f32>().unwrap_or(1.0));
        [0; 3].map(|_| c.next().unwrap_or(1.0))
    }

    /// The names over heads (`cod4_names::quads`) of `names`, projected
    /// through the drawn view `eye`: each its client's name
    /// (`CL_GetClientName`) in his colour relative to us
    /// (`CG_RelativeTeamColor`) with his rank's icon and level off
    /// `mp/rankIconTable.csv` and `mp/rankTable.csv`; nothing before the
    /// zones load (cod4-hud.md 17.1).
    pub(crate) fn overhead_names(
        &self,
        s: &Cod4Session,
        names: &[crate::cod4_names::Name],
        eye: &crate::cod4_waypoints::Eye,
        dv: &crate::cod4_names::Dvars,
    ) -> Vec<HudQuad> {
        use crate::cod4_names::{Look, Relation, quads};
        use crate::play::cod4::{client_team, client_u32};
        let mut out = Vec::new();
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return out;
        };
        let sp = ScreenPlacement::cod4(eye.size[0], eye.size[1]);
        let font = l.fonts.pick(dv.font, 1.0, &sp);
        let client = |n: u32| snap.clients.iter().find(|c| c.0[0] == n);
        let me = s.gamestate().map_or(u32::MAX, |g| g.client_num as u32);
        let mine = client(me).map_or(0, client_team);
        for n in names {
            let Some(c) = client(n.client) else {
                continue;
            };
            let text = client_name(c);
            let rank = client_u32(c, "rank") as i32;
            let prestige = client_u32(c, "prestige") as i32;
            let look = Look {
                font,
                text: &text,
                rgb: self.team_rgb(s, Relation::of(n.client, client_team(c), me, mine).dvar()),
                rank_icon: l
                    .rank_icons
                    .get(&(rank, prestige))
                    .filter(|m| !m.is_empty())
                    .and_then(|m| image_of(l, m)),
                rank_level: l.rank_levels.get(&rank).cloned(),
            };
            quads(n, &look, eye, dv, &mut out);
        }
        out
    }

    /// The `low health` menu's owner draw 112: its background material
    /// over its rect in its colour, at the pulse's alpha
    /// (`CG_DrawPlayerLowHealthOverlay`, 0x442e70).
    pub(super) fn draw_low_health(
        &self,
        l: &Loaded,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let alpha = self.damage.overlay_alpha(now);
        let Some(layout) = l
            .script_menus
            .get(LOW_HEALTH_MENU)
            .and_then(|m| m.layout.as_ref())
            .filter(|_| alpha != 0.0)
        else {
            return;
        };
        for it in layout
            .items
            .iter()
            .filter(|it| it.owner_draw == OD_LOW_HEALTH)
        {
            let Some(tex) = it.background.as_deref().and_then(|m| image_of(l, m)) else {
                continue;
            };
            let [x, y, w, h] = it.rect;
            let (rx, rw) = sp.axis(0, x, w, it.align[0]);
            let (ry, rh) = sp.axis(1, y, h, it.align[1]);
            let mut rgba = it.fore_color;
            rgba[3] = alpha;
            out.push(quad(rx, ry, rw, rh, rgba, tex, false));
        }
    }

    /// The low-health overlay's alpha at `now`, what the live gate reads.
    #[cfg(test)]
    pub(crate) fn low_health_alpha(&self, now: i32) -> f32 {
        self.damage.overlay_alpha(now)
    }

    /// The last hit's damage kick (cod4-client-view.md 14).
    pub(crate) fn damage_kick(&self) -> crate::play::cod4_kick::DamageKick {
        self.damage.kick()
    }

    /// The hit direction slots, what the live gate reads.
    #[cfg(test)]
    pub(crate) fn damage_slots(&self) -> [crate::cod4_damage::Icon; 8] {
        *self.damage.icons()
    }

    /// The hit direction icons (`cod4_damage`) around the screen's centre
    /// for a view at `yaw`, at drawn time `now`; none under a sight's
    /// overlay or a shellshock's screen blend, or before the zones load.
    pub(crate) fn damage_icons(
        &self,
        s: &Cod4Session,
        ps: &crate::cod4_viewmodel::ViewPs,
        size: (f32, f32),
        now: i32,
        yaw: f32,
    ) -> Vec<HudQuad> {
        let Some(Some(l)) = &self.loaded else {
            return Vec::new();
        };
        let Some(tex) = image_of(l, crate::cod4_damage::HIT_DIRECTION) else {
            return Vec::new();
        };
        let scoped = s
            .gamestate()
            .map(crate::play::cod4::weapon_list)
            .and_then(|list| {
                let i = (ps.view_weapon() as usize).checked_sub(1)?;
                l.weapon_hud.get(list.get(i)?)
            })
            .is_some_and(|def| crate::cod4_crosshair::overlay_up(ps, def));
        let (parms, start, duration) = crate::play::cod4_shock::of_session(s);
        let flashed = crate::play::cod4_shock::flashbanged(parms.as_ref(), start, duration, now);
        let sp = ScreenPlacement::cod4(size.0, size.1);
        self.damage
            .icon_quads(&sp, now, yaw, scoped || flashed, &tex)
    }

    /// The waypoint hudelems in the world (`CG_AddDrawSurfsFor3dHudElems`,
    /// each one `iw3mp.exe` 0x43a190..0x43a2b4): an element of kind 13
    /// with an alpha, no offscreen material and a material is a sprite at
    /// `(x, y, z)` in its lerped colour, its height (scaled from
    /// `fromHeight` over `scaleTime`) as its world radius, or with a
    /// `value` above 0 that height times 0.0043 of the screen
    /// (cod4-hud.md 16). Bomb Squad's `waypoint_bombsquad` icons are these.
    pub(crate) fn world_quads(
        &self,
        s: &Cod4Session,
        view: &crate::cod2_world_hud::WorldView,
        now: i32,
    ) -> Vec<HudQuad> {
        use crate::cod2_world_hud::{CONSTANT_SCALE, Size};
        let mut out = Vec::new();
        let Some(Some(l)) = &self.loaded else {
            return out;
        };
        let Some(snap) = s.client().snapshots.last() else {
            return out;
        };
        let mut elems: Vec<HudElem> = h::hud_elems(&snap.ps)
            .into_iter()
            .filter(|e| e.kind == h::kind::WAYPOINT && e.offscreen_material == 0)
            .collect();
        elems.sort_by(|a, b| a.sort.total_cmp(&b.sort));
        for e in &elems {
            let rgba = colour(e.color, e.from_color, e.fade_start_time, e.fade_time, now);
            if rgba[3] <= 0.0 || e.material == 0 {
                continue;
            }
            let name = self.configstring(s, h::CS_MATERIALS + e.material as u16);
            let Some(tex) = image_of(l, &name) else {
                continue;
            };
            let height = material_size(e, now).1;
            if height == 0.0 {
                continue;
            }
            let size = if e.value > 0.0 {
                Size::Constant(height * CONSTANT_SCALE)
            } else {
                Size::World {
                    radius: height,
                    min: 0.0,
                }
            };
            out.extend(view.sprite(glam::Vec3::new(e.x, e.y, e.z), size, rgba, &tex));
        }
        out
    }

    /// The waypoints with an offscreen material, in the 2D pass
    /// (`iw3mp.exe` 0x439a40, `cod4_waypoints`): each at its
    /// target entity when it names one in the snapshot (a player raised
    /// by his stance), else at its own point; none for our own entity
    /// (cod4-hud.md 16.3).
    pub(crate) fn offscreen_waypoints(
        &self,
        s: &Cod4Session,
        eye: &crate::cod4_waypoints::Eye,
        now: i32,
    ) -> Vec<HudQuad> {
        use crate::cod4_waypoints::{POINTER, dvar::PLAYER_OFFSET};
        let mut out = Vec::new();
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return out;
        };
        let own = snap.ps.word(vcod_cod4::ps_word!("clientNum"));
        let pointer = image_of(l, POINTER);
        let mut elems: Vec<HudElem> = h::hud_elems(&snap.ps)
            .into_iter()
            .filter(|e| e.kind == h::kind::WAYPOINT && e.offscreen_material != 0)
            .collect();
        elems.sort_by(|a, b| a.sort.total_cmp(&b.sort));
        for e in &elems {
            let rgba = colour(e.color, e.from_color, e.fade_start_time, e.fade_time, now);
            if rgba[3] <= 0.0 {
                continue;
            }
            let name = self.configstring(s, h::CS_MATERIALS + e.offscreen_material as u16);
            let Some(tex) = image_of(l, &name) else {
                continue;
            };
            let at = if e.target_ent as u32 == ENTITYNUM_NONE {
                glam::Vec3::new(e.x, e.y, e.z)
            } else if e.target_ent as u32 == own {
                continue;
            } else {
                let Some(t) = snap
                    .entities
                    .iter()
                    .find(|x| x.number() == e.target_ent as u32)
                else {
                    continue;
                };
                let mut at = crate::play::cod4::origin_of(t);
                if t.etype() == crate::play::cod4::ET_PLAYER {
                    let f = crate::play::cod4::ent_u32(t, "lerp.eFlags");
                    at.z += match f {
                        f if f & 8 != 0 => PLAYER_OFFSET[0],
                        f if f & 4 != 0 => PLAYER_OFFSET[1],
                        _ => PLAYER_OFFSET[2],
                    };
                }
                at
            };
            crate::cod4_waypoints::quads(eye, at, rgba, &tex, pointer.as_deref(), &mut out);
        }
        out
    }
}
