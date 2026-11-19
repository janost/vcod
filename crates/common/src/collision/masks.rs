//! The contents, surface and trace mask bits.

/// Shared with the xmodel collision surfaces, which use the same bits.
pub const CONTENTS_SOLID: u32 = 0x1;
/// Window panes (`glass@brokenwindow`, `dam_window`: 0x2090, 0x8000010,
/// 0x28000010 over the stock maps) and lamp glass on xmodels. In retail's
/// player mask, so a player stops at a window and a bullet does not
/// (docs/research/bsp-ibsp59-format.md, "Content flags").
pub const CONTENTS_GLASS: u32 = 0x10;
pub(super) const CONTENTS_PLAYERCLIP: u32 = 0x10000;
/// Census-proven water bit: docs/research/bsp-ibsp59-format.md, "Content flags".
pub const CONTENTS_WATER: u32 = 0x20;
/// The mask `G_RunItem` (game.mp 0x4ec47) hands `trap_PointContents` before
/// it frees a landing item (docs/research/cod11-items.md, section 8). No
/// stock material carries the bit.
pub const CONTENTS_NODROP: u32 = 0x8000_0000;
/// Ladder-climb flag on brush materials; pmove grabs ladders from trace hits carrying it.
pub const SURF_LADDER: u32 = 0x8;
/// `surfaceparm slick` (CoDMP.exe surfaceparm table, 0x571a40); no stock map
/// carries it (docs/research/cod11-mantle.md, "Slick ground").
pub const SURF_SLICK: u32 = 0x2;
/// Brushless terrain is the one collidable material word without SOLID or
/// PLAYERCLIP (bsp-ibsp59-format.md, "Content flags").
pub(super) const CONTENTS_TERRAIN: u32 = 0x4;

/// Retail's player tracemask, the `pm->tracemask` `ClientThink_real` stores
/// for a live player (docs/research/cod11-mantle.md, "The player is a
/// capsule"): SOLID, GLASS, PLAYERCLIP and two bits (0x800000, 0x2000000)
/// no stock material carries.
pub const MASK_PLAYERSOLID: u32 = 0x2810011;
/// The mask `Bullet_Fire_Extended` (game.mp 0x78890) hands
/// `trap_LocationalTrace`: SOLID, GLASS, WATER, 0x2000 (the kerb and floor
/// brushes' 0x2080 word) and the same two high bits. No PLAYERCLIP, so a
/// masked wire fence stops a player and not a bullet.
pub const MASK_SHOT: u32 = 0x2802031;
/// `CanDamage`'s (0x5a098) mask for a blast's five probes: the shot mask
/// with 0x80 for 0x20.
pub const MASK_BLAST: u32 = 0x2802091;
/// SOLID and GLASS: the `trap_Trace` mask of a missile's explode trace
/// down, its ground snap and `grenadeExplosionEffect` (combat doc 12.6,
/// 13.2). Not the move trace's: that is [`MASK_MISSILE_MOVE`].
pub const MASK_MISSILE: u32 = CONTENTS_SOLID | CONTENTS_GLASS;
/// The clipmask `fire_grenade` and `fire_rocket` store and `G_RunMissile`'s
/// move trace reads (1.1d 0x544a0, 0x546ef, read at 0x5409a; 1.5b 0x5539d,
/// 0x556bd; UO 0x5f7ad), the blast mask's value: it meets the 0x2080 kerb
/// brushes too (combat doc 11.1, 12.1).
pub const MASK_MISSILE_MOVE: u32 = MASK_BLAST;
/// The brush contents that enter the clip as brushes: every bit a trace
/// mask meets, the item masks' 0x80 and 0x400 included (cod11-items.md,
/// section 14.2), but water, which stays a volume. Glass carries no SOLID
/// bit (mp_depot's panes are 0x8000010), and the 0x2080 kerb and floor
/// brushes carry neither SOLID nor PLAYERCLIP, so a player walks through
/// what stops a bullet, a blast probe and a falling item.
/// United Offensive's LMG mount brushes (0x404000) enter too, for the
/// deploy's point probe (`pmove::CONTENTS_HMG`); no mask a trace runs with
/// meets them. So do its vehicle clip brushes (0x200), which only a
/// vehicle's clipmask 0x201 meets (docs/research/coduo-vehicles.md 12).
pub(super) const CLIP_BRUSH: u32 =
    (MASK_PLAYERSOLID | MASK_SHOT | MASK_BLAST | 0x491 | 0x40_4000 | CONTENTS_VEHICLE_CLIP)
        & !CONTENTS_WATER;
/// United Offensive's vehicle clip brush contents.
pub const CONTENTS_VEHICLE_CLIP: u32 = 0x200;
pub(super) const TRACE_MASK_MOVE: u32 = MASK_PLAYERSOLID;
pub(super) const TRACE_MASK_SHOT: u32 = MASK_SHOT;

/// [`CollisionWorld`]'s `model_contents` for a model no entity linked.
pub(super) const NO_ENTITY_CONTENTS: u64 = u64::MAX;
