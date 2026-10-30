//! CoD2's use-trigger team and claim builtins, which `sd`'s bomb zones
//! call (cod2-gsc-diff.md 5.3). Each takes a `trigger_use` or
//! `trigger_use_touch` and writes the two bytes the use list reads
//! (`game::cod2_use`). Addresses are cod2_lnxded's, VERIFIED.

use super::client::client_receiver;
use super::entity::entity_receiver;
use super::score::Builtin;
use crate::game::host::GameHost;
use crate::game::trigger::{Trigger, TriggerKind};
use vcod_gsc::{Cx, EntId, ErrorKind, Target, Value};

pub const NAMES: &[(&str, Builtin)] = &[
    ("setteamfortrigger", set_team_for_trigger),
    ("clientclaimtrigger", client_claim_trigger),
    ("clientreleasetrigger", client_release_trigger),
    ("releaseclaimedtrigger", release_claimed_trigger),
];

/// The use trigger `id`, else the handlers' `trigger entity must be of
/// type trigger_use or trigger_use_touch` error.
fn use_trigger(host: &mut GameHost, id: EntId) -> Result<&mut Trigger, ErrorKind> {
    host.triggers
        .get_mut(id)
        .filter(|t| t.kind == TriggerKind::Use)
        .ok_or(ErrorKind::BadType(
            "trigger entity must be of type trigger_use or trigger_use_touch",
        ))
}

fn entity_arg(args: &[Value]) -> Result<EntId, ErrorKind> {
    match args.first() {
        Some(Value::Entity(id)) => Ok(*id),
        _ => Err(ErrorKind::BadType("not an entity")),
    }
}

/// `trigger setTeamForTrigger(team)` (`GScr_SetTeamForTrigger` 0x8117944):
/// `allies` 2, `axis` 1, `none` 0 into `ent+0x167`; any other string an
/// error.
pub(super) fn set_team_for_trigger(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let team = match args.first() {
        Some(Value::String(s)) => match cx.resolve(*s) {
            "allies" => 2,
            "axis" => 1,
            "none" => 0,
            _ => {
                return Err(ErrorKind::BadType(
                    "setteamfortrigger: invalid team used must be allies, axis or none",
                ));
            }
        },
        _ => return Err(ErrorKind::BadType("setteamfortrigger takes a team")),
    };
    use_trigger(host, id)?.team = team;
    Ok(Value::Undefined)
}

/// `player clientClaimTrigger(trigger)` (`GScr_ClientClaimTrigger`
/// 0x8117a72): the claim moves to the player only when the trigger is
/// unclaimed or already his.
pub(super) fn client_claim_trigger(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)
        .map_err(|_| ErrorKind::BadType("clientclaimtrigger: claimer must be a client."))?;
    let t = use_trigger(host, entity_arg(args)?)?;
    if t.claimer.is_none_or(|c| usize::from(c) == slot) {
        t.claimer = Some(slot as u16);
    }
    Ok(Value::Undefined)
}

/// `player clientReleaseTrigger(trigger)` (`GScr_ClientReleaseTrigger`
/// 0x8117b5a): the claim goes only when it is the player's.
pub(super) fn client_release_trigger(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)
        .map_err(|_| ErrorKind::BadType("clientreleasetrigger: releaser must be a client."))?;
    let t = use_trigger(host, entity_arg(args)?)?;
    if t.claimer.is_some_and(|c| usize::from(c) == slot) {
        t.claimer = None;
    }
    Ok(Value::Undefined)
}

/// `trigger releaseClaimedTrigger()` (`GScr_ReleaseClaimedTrigger`
/// 0x8117c28): the claim goes whoever holds it.
pub(super) fn release_claimed_trigger(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    use_trigger(host, id)?.claimer = None;
    Ok(Value::Undefined)
}
