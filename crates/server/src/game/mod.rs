//! The game module: the script runtime and the CoD-specific host the VM
//! calls into. `server.rs` keeps the engine side.

pub mod artillery;
pub mod bodies;
pub mod builtins;
pub mod cod2;
#[cfg(test)]
mod cod2_antilag_ab;
pub mod cod2_archive;
pub mod cod2_blast;
#[cfg(test)]
mod cod2_boot;
pub mod cod2_cmds;
pub mod cod2_combat;
#[cfg(test)]
mod cod2_combat_ab;
pub mod cod2_corpse;
#[cfg(test)]
mod cod2_fight;
#[cfg(test)]
mod cod2_gametypes;
#[cfg(test)]
mod cod2_gaps;
#[cfg(test)]
mod cod2_hitpose_ab;
pub mod cod2_identify;
pub mod cod2_items;
#[cfg(test)]
mod cod2_killcam_ab;
#[cfg(test)]
mod cod2_ladder;
pub mod cod2_objectives;
#[cfg(test)]
mod cod2_pickup_ab;
pub mod cod2_turret;
#[cfg(test)]
mod cod2_turret_ab;
pub mod cod2_use;
pub mod cod2_wire;
pub mod cod4;
pub mod combat;
pub mod commands;
pub mod corpse;
pub mod damage_trigger;
pub mod entity;
pub mod fields;
pub mod hitrig;
pub mod host;
pub mod item;
pub mod link;
pub mod mantle_block;
pub mod missile;
pub mod mover;
pub mod pickup;
pub mod profile;
pub mod say;
pub mod scoreboard;
pub mod script;
pub mod slot;
pub mod spawn;
pub mod stuck;
pub mod teams;
#[cfg(test)]
mod teams_ab;
pub mod temp_entity;
pub mod timeout;
pub mod trigger;
pub mod turret;
pub mod usage;
pub mod use_list;
pub mod vehicle;
pub mod vote;
pub mod wire;

/// `ClientThink_real`'s hold on a cmd's clock: at most 1000 ms behind and
/// 200 ms ahead of `level_time` (CoD1 player-clip doc 8.12, cod2-movement.md
/// 21, CoD4's twin). Widened so neither bound wraps: past `i32::MAX - 200`
/// the plain clamp's bounds cross and it panics.
pub fn hold_cmd_time(server_time: i32, level_time: i32) -> i32 {
    let level = i64::from(level_time);
    i64::from(server_time).clamp(level - 1000, level + 200) as i32
}

/// `testing::fixture` with a two-submodel world attached, for the paths that
/// read brush bounds. Model 1 is a 32x32x72 box, model 2 a 64-unit cube;
/// both are the shapes the trigger tests assert against.
#[cfg(test)]
pub fn testing_world_fixture() -> (vcod_gsc::Vm, host::GameHost) {
    let (vm, mut host) = testing::fixture();
    host.model_bounds = vec![
        ([0.0; 3], [0.0; 3]),
        ([-16.0, -16.0, 0.0], [16.0, 16.0, 72.0]),
        ([-32.0; 3], [32.0; 3]),
    ];
    (vm, host)
}

#[cfg(test)]
pub mod testing {
    use crate::game::host::GameHost;
    use vcod_gsc::Vm;

    /// A VM and a host with an empty object table, for the game module's tests.
    pub fn fixture() -> (Vm, GameHost) {
        (
            Vm::new(),
            GameHost::new(&crate::game::profile::COD1_11, vec![String::new(); 2048]),
        )
    }
}
