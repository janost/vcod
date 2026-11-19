//! `GameHost::client_commands`: the per-client server commands the game
//! queued, each stamped with [`crate::configstrings::next_write_seq`] as
//! the configstring writes are, so an engine that sends both as written
//! (CoD2's `SV_SetConfigstring` and `SV_GameSendServerCommand`,
//! cod2-map-cycle.md 3) can put them back in one order.

use super::slot::ClientSlot;

/// Who a queued server command goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recipient {
    /// `SV_GameSendServerCommand(-1, ..)`: the engine fans it out to every
    /// client past `CS_CONNECTED`. CoD2's and CoD4's builtins and votes
    /// queue it; vcod's CoD1 server never sees one.
    All,
    One(ClientSlot),
}

/// The client number `SV_GameSendServerCommand` takes: -1 for every client.
impl std::fmt::Display for Recipient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Recipient::All => f.write_str("-1"),
            Recipient::One(slot) => slot.fmt(f),
        }
    }
}

impl From<ClientSlot> for Recipient {
    fn from(slot: ClientSlot) -> Self {
        Recipient::One(slot)
    }
}

/// `slot` as a recipient, for tests that name slots by number.
#[cfg(test)]
pub fn one(slot: usize) -> Recipient {
    Recipient::One(ClientSlot::at(slot))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientCommands {
    list: Vec<(Recipient, String)>,
    seqs: Vec<u64>,
}

impl ClientCommands {
    pub fn push(&mut self, to: impl Into<Recipient>, text: String) {
        self.list.push((to.into(), text));
        self.seqs.push(crate::configstrings::next_write_seq());
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.seqs.clear();
    }

    /// The commands from `at` on, taken out.
    pub fn split_off(&mut self, at: usize) -> Vec<(Recipient, String)> {
        self.seqs.truncate(at);
        self.list.split_off(at)
    }

    pub fn take(&mut self) -> Vec<(Recipient, String)> {
        self.seqs.clear();
        std::mem::take(&mut self.list)
    }

    /// Every command with its write stamp, oldest first.
    pub fn take_stamped(&mut self) -> Vec<(u64, Recipient, String)> {
        let seqs = std::mem::take(&mut self.seqs);
        seqs.into_iter()
            .zip(std::mem::take(&mut self.list))
            .map(|(s, (to, text))| (s, to, text))
            .collect()
    }
}

impl std::ops::Deref for ClientCommands {
    type Target = [(Recipient, String)];
    fn deref(&self) -> &Self::Target {
        &self.list
    }
}

impl PartialEq<Vec<(Recipient, String)>> for ClientCommands {
    fn eq(&self, other: &Vec<(Recipient, String)>) -> bool {
        self.list == *other
    }
}

impl<const N: usize> PartialEq<[(Recipient, String); N]> for ClientCommands {
    fn eq(&self, other: &[(Recipient, String); N]) -> bool {
        self.list == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> ClientCommands {
        let mut c = ClientCommands::default();
        for (to, text) in [(one(0), "a"), (one(1), "b"), (Recipient::All, "c")] {
            c.push(to, text.to_string());
        }
        c
    }

    /// `split_off` cuts the stamps where it cuts the commands, so the kept
    /// head still takes its own stamps, in write order.
    #[test]
    fn split_off_keeps_the_stamps_beside_their_commands() {
        let mut c = three();
        let stamps: Vec<u64> = three().take_stamped().iter().map(|s| s.0).collect();
        assert!(stamps.windows(2).all(|w| w[0] < w[1]));
        let tail = c.split_off(1);
        assert_eq!(
            tail,
            vec![(one(1), "b".to_string()), (Recipient::All, "c".to_string())]
        );
        c.push(one(2), "d".to_string());
        let kept = c.take_stamped();
        assert_eq!(kept.len(), 2);
        assert_eq!((kept[0].1, kept[0].2.as_str()), (one(0), "a"));
        assert_eq!((kept[1].1, kept[1].2.as_str()), (one(2), "d"));
        assert!(kept[0].0 < kept[1].0);
        assert!(c.is_empty());
        assert!(c.take_stamped().is_empty());
    }

    /// `take` and `clear` empty both halves: a later push stands alone.
    #[test]
    fn take_and_clear_drop_the_stamps_too() {
        for drain in [
            |c: &mut ClientCommands| drop(c.take()),
            |c: &mut ClientCommands| c.clear(),
        ] {
            let mut c = three();
            drain(&mut c);
            c.push(one(5), "e".to_string());
            let s = c.take_stamped();
            assert_eq!(s.len(), 1);
            assert_eq!((s[0].1, s[0].2.as_str()), (one(5), "e"));
        }
    }
}
