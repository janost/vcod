//! Playing on a server: the stock menus, the usercmd stream, prediction and
//! the first-person weapon.

pub mod clock;
pub mod cmds;
pub mod cod1_soak;
pub mod cod2;
#[cfg(test)]
mod cod2_census;
pub mod cod2_events;
pub mod cod2_fx;
pub mod cod2_gun;
pub mod cod2_input;
pub mod cod2_net;
pub mod cod2_predict;
pub mod cod2_recoil;
pub mod cod2_shock;
#[cfg(test)]
mod cod2_soak;
pub mod cod2_sound;
pub mod cod2_view;
pub mod cod4;
pub mod cod4_input;
pub mod cod4_kick;
pub mod cod4_menus;
pub mod cod4_players;
pub mod cod4_predict;
pub mod cod4_profile;
pub mod cod4_shock;
#[cfg(test)]
mod cod4_sweep;
pub mod cvars;
pub mod demos;
pub mod events;
pub mod input;
pub mod join;
#[cfg(test)]
mod loopback;
pub mod predict;
pub mod quake;
pub mod recoil;
#[cfg(test)]
mod replay;
pub mod seat;
pub mod shock;
pub mod view;
