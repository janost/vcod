//! [`ClientSlot`]: a client's slot, checked where it is made. The engine and
//! the level index their client arrays by plain `usize`; a builtin or a
//! queued server command takes a `ClientSlot`, so a script call on an entity
//! that is not a player cannot address a slot past the clients.

use super::entity::ObjectTable;
use crate::server::MAX_CLIENTS;
use vcod_gsc::EntId;

/// A client slot below the level's client count. A client's entity number
/// is its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientSlot(u8);

impl ClientSlot {
    /// `slot` if it is below `count`, the level's client count
    /// (`GameHost::client_slot` passes the engine's).
    pub fn new(slot: usize, count: usize) -> Option<Self> {
        let count = count.min(MAX_CLIENTS);
        (slot < count).then_some(Self(slot as u8))
    }

    /// The slot of `id` if it is a live entity with a `gclient_t`
    /// (`ObjectTable::spawn_client` keeps those below `MAX_CLIENTS`).
    pub fn of_entity(ents: &ObjectTable, id: EntId) -> Option<Self> {
        ents.get(id)
            .filter(|e| e.client.is_some())
            .and_then(|_| Self::new(id.0 as usize, MAX_CLIENTS))
    }

    /// The slot as an index into a client array.
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

#[cfg(test)]
impl ClientSlot {
    /// Slot `n`, for tests that name slots by number.
    pub fn at(n: usize) -> Self {
        Self::new(n, MAX_CLIENTS).expect("a client slot")
    }
}

impl std::fmt::Display for ClientSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_refuses_a_slot_at_or_past_the_count() {
        assert_eq!(ClientSlot::new(7, 8).map(ClientSlot::index), Some(7));
        assert_eq!(ClientSlot::new(8, 8), None);
        assert_eq!(ClientSlot::new(64, 1000), None);
    }

    /// A script entity's number is no slot, nor is a freed client's.
    #[test]
    fn of_entity_takes_only_a_client() {
        let (mut vm, mut host) = crate::game::testing_world_fixture();
        vm.with_cx(|cx| {
            let player = host.ents.spawn_client(cx, 3, None).unwrap();
            let other = host.ents.spawn(cx).unwrap();
            assert_eq!(
                ClientSlot::of_entity(&host.ents, player).map(ClientSlot::index),
                Some(3)
            );
            assert_eq!(ClientSlot::of_entity(&host.ents, other), None);
        });
    }

    /// A bare slot past the engine's clients queues nothing: CoD1's send
    /// indexes its client table by it.
    #[test]
    fn command_to_drops_a_slot_past_the_engine_clients() {
        let (_vm, mut host) = crate::game::testing::fixture();
        host.engine_slots = 8;
        host.command_to(8, "x".into());
        assert!(host.client_commands.is_empty());
        host.command_to(7, "y".into());
        assert_eq!(
            host.client_commands,
            [(crate::game::commands::one(7), "y".to_string())]
        );
    }
}
