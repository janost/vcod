//! Damage the script asked for, queued rather than dispatched.
//!
//! Retail runs `CodeCallback_PlayerDamage` inline from `radiusDamage`: a
//! builtin calls back into script. `Cx` deliberately has no route back into
//! the VM, for the same reason `notify` is queued, so damage becomes an
//! event the server drains after `run_frame`. Stage 6 does the draining;
//! stage 2 only fills the queue. The divergence is observable and is listed
//! in docs/research/cod11-gsc-language.md section 10.

use vcod_gsc::EntId;

pub struct DamageEvent {
    pub origin: [f32; 3],
    pub radius: f32,
    pub max_damage: f32,
    pub min_damage: f32,
    /// `None` until stage 6 gives `radiusDamage` an attacker argument.
    pub attacker: Option<EntId>,
}
