//! `GameHost::client_commands`: the per-client server commands the game
//! queued, each stamped with [`crate::configstrings::next_write_seq`] as
//! the configstring writes are, so an engine that sends both as written
//! (CoD2's `SV_SetConfigstring` and `SV_GameSendServerCommand`,
//! cod2-map-cycle.md 3) can put them back in one order.

/// The slot a command to every client goes to (`SV_GameSendServerCommand(-1,
/// ..)`): the engine fans it out to every client past `CS_CONNECTED`.
/// Only CoD2's builtins queue it; vcod's CoD1 server never sees one.
pub const ALL_CLIENTS: usize = usize::MAX;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientCommands {
    list: Vec<(usize, String)>,
    seqs: Vec<u64>,
}

impl ClientCommands {
    pub fn push(&mut self, cmd: (usize, String)) {
        self.list.push(cmd);
        self.seqs.push(crate::configstrings::next_write_seq());
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.seqs.clear();
    }

    /// The commands from `at` on, taken out.
    pub fn split_off(&mut self, at: usize) -> Vec<(usize, String)> {
        self.seqs.truncate(at);
        self.list.split_off(at)
    }

    pub fn take(&mut self) -> Vec<(usize, String)> {
        self.seqs.clear();
        std::mem::take(&mut self.list)
    }

    /// Every command with its write stamp, oldest first.
    pub fn take_stamped(&mut self) -> Vec<(u64, usize, String)> {
        let seqs = std::mem::take(&mut self.seqs);
        seqs.into_iter()
            .zip(std::mem::take(&mut self.list))
            .map(|(s, (to, text))| (s, to, text))
            .collect()
    }
}

impl std::ops::Deref for ClientCommands {
    type Target = [(usize, String)];
    fn deref(&self) -> &Self::Target {
        &self.list
    }
}

impl PartialEq<Vec<(usize, String)>> for ClientCommands {
    fn eq(&self, other: &Vec<(usize, String)>) -> bool {
        self.list == *other
    }
}

impl<const N: usize> PartialEq<[(usize, String); N]> for ClientCommands {
    fn eq(&self, other: &[(usize, String); N]) -> bool {
        self.list == other
    }
}
