//! `SV_AddServerCommand` on one client's command ring, as CoD1 1.1
//! (`cod_lnxded` 0x808b680, protocol-1.1.md, "The server command queue"),
//! CoD2 1.3 (cod2_lnxded 0x8094750, cod2-protocol.md 3.4) and CoD4 1.7
//! (0x81769b2) run it: one squash and jump table, the thresholds and the
//! letters any later command replaces per game ([`Rules`]). Each game reads
//! a command's type off its letter, which differs between them.

use super::slot::Link;

/// What one game's `SV_AddServerCommand` differs in.
pub struct Rules {
    /// Commands past the client's ack beyond which its unsent type 0
    /// commands go.
    pub pending: i32,
    /// The letters any later command of the same letter replaces.
    pub replace_any: &'static [u8],
}

/// CoD1's 64-slot ring: 31 pending (`cod_lnxded` 0x808b6aa), and `a b o
/// p q r t` (`SV_FindPendingCommand`'s jump table at 0x80d5820).
pub const COD1: Rules = Rules {
    pending: 31,
    replace_any: b"abopqrt",
};

/// CoD2's and CoD4's 128-slot ring: 63 pending (CoD2 0x809477d, CoD4
/// 0x81769df) and `C D a b o p q r t` (the jump tables at CoD2 0x814b3c0
/// and CoD4 0x82326a8).
pub const COD2: Rules = Rules {
    pending: 63,
    replace_any: b"CDabopqrt",
};

/// What [`queue`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queued {
    Added,
    /// An unsent match was taken out and this one appended.
    Replaced,
    /// A type 0 command the client is not ready for.
    Dropped,
    /// Queueing would overwrite a slot the client has not acked; nothing
    /// was written, the caller drops the client.
    Overflow,
}

/// [`queue`] under [`COD2`], for CoD2 and CoD4: false on
/// [`Queued::Overflow`].
pub fn add<F>(
    link: &mut Link<F>,
    kept: &mut [bool],
    active: bool,
    text: &str,
    reliable: bool,
) -> bool {
    queue(&COD2, link, kept, active, text, reliable) != Queued::Overflow
}

/// Queues `text` on `link`'s ring; `kept` holds each slot's type (true for
/// type 1, `SV_CMD_RELIABLE`; false for type 0, `SV_CMD_CAN_IGNORE`).
/// - A client not `CS_ACTIVE`, or more than [`Rules::pending`] commands
///   past its ack, first loses its unsent type 0 commands (CoD2
///   0x8094660), and a type 0 command is then not queued.
/// - A command takes the place of the first unsent type 1 one it
///   [`supersedes`]: that one leaves the ring and the new one goes last.
/// - Overflow when the ring would hold more unacked commands than it has
///   slots (retail's `== 0x81` test, CoD2 0x8094874).
pub fn queue<F>(
    rules: &Rules,
    link: &mut Link<F>,
    kept: &mut [bool],
    active: bool,
    text: &str,
    reliable: bool,
) -> Queued {
    let pending = link.netchan.reliable_sequence as i32 - link.reliable_ack;
    if !active || pending > rules.pending {
        squash(link, kept);
        if !reliable {
            return Queued::Dropped;
        }
    }
    let ring = kept.len();
    let stale = unsent(link)
        .map(|seq| slot(seq, ring))
        .position(|s| kept[s] && supersedes(rules, text, &link.netchan.reliable[s]));
    if let Some(i) = stale {
        edit_unsent(link, kept, |cmds| drop(cmds.remove(i)));
    }
    if !append(link, kept, text, reliable) {
        Queued::Overflow
    } else if stale.is_some() {
        Queued::Replaced
    } else {
        Queued::Added
    }
}

/// `text` on the end of the ring with no squash or replace; false, and
/// nothing written, when the ring is full of unacked commands.
pub fn append<F>(link: &mut Link<F>, kept: &mut [bool], text: &str, reliable: bool) -> bool {
    let ring = kept.len();
    let seq = link.netchan.reliable_sequence as i32 + 1;
    if seq - link.reliable_ack > ring as i32 {
        return false;
    }
    link.netchan.reliable_sequence = seq as u32;
    let s = slot(seq, ring);
    link.netchan.reliable[s] = text.to_string();
    kept[s] = reliable;
    true
}

/// The squash: the unsent type 0 commands leave the ring, the rest close
/// up.
pub fn squash<F>(link: &mut Link<F>, kept: &mut [bool]) {
    edit_unsent(link, kept, |cmds| cmds.retain(|(_, kept)| *kept));
}

/// Whether `new` replaces the unsent type 1 `old` (`SV_FindPendingCommand`,
/// CoD1 0x808b580, CoD2 0x8094530, CoD4 0x8176a76..0x8176bff): one letter,
/// not a configstring chunk `x`..`z` (CoD2 0x80945ab, CoD4 0x8176abe), and
/// then the same text after it (CoD2 0x80945dc, CoD4 0x8176adb), or a
/// letter in [`Rules::replace_any`], or a `d`/`v` naming the same first
/// word (CoD2 0x80944aa, CoD4 0x8176b85).
pub fn supersedes(rules: &Rules, new: &str, old: &str) -> bool {
    let (new, old) = (new.as_bytes(), old.as_bytes());
    let Some((&letter, rest)) = new.split_first() else {
        return false;
    };
    if old.first() != Some(&letter) || (b'x'..=b'z').contains(&letter) {
        return false;
    }
    fn first_word(t: &[u8]) -> Option<&[u8]> {
        t.get(2..).and_then(|r| r.split(|&b| b == b' ').next())
    }
    rest == &old[1..]
        || rules.replace_any.contains(&letter)
        || matches!(letter, b'd' | b'v') && first_word(new) == first_word(old)
}

/// A sequence number's slot in a ring of `ring` (a power of two).
fn slot(seq: i32, ring: usize) -> usize {
    seq as usize & (ring - 1)
}

/// The sequence numbers no message has carried yet:
/// `reliableSent + 1 ..= reliableSequence`.
fn unsent<F>(link: &Link<F>) -> std::ops::RangeInclusive<i32> {
    link.reliable_sent + 1..=link.netchan.reliable_sequence as i32
}

/// Takes the unsent commands (text, type 1) out of the ring, lets `edit`
/// drop some, and packs the rest back from `reliableSent + 1`, the
/// sequence ending on the last of them.
fn edit_unsent<F>(
    link: &mut Link<F>,
    kept: &mut [bool],
    edit: impl FnOnce(&mut Vec<(String, bool)>),
) {
    let ring = kept.len();
    let mut cmds: Vec<(String, bool)> = unsent(link)
        .map(|seq| slot(seq, ring))
        .map(|s| (std::mem::take(&mut link.netchan.reliable[s]), kept[s]))
        .collect();
    edit(&mut cmds);
    let mut seq = link.reliable_sent;
    for (text, k) in cmds {
        seq += 1;
        link.netchan.reliable[slot(seq, ring)] = text;
        kept[slot(seq, ring)] = k;
    }
    link.netchan.reliable_sequence = seq as u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_supersedes_what() {
        for rules in [&COD1, &COD2] {
            // `d` matches on the index alone, as `v` on the name.
            assert!(supersedes(rules, "d 5 118", "d 5 117"));
            assert!(!supersedes(rules, "d 5 118", "d 51 118"));
            // These letters on the letter alone.
            assert!(supersedes(rules, "t 3", "t 1"));
            assert!(supersedes(rules, "b 0 1 2", "b 9"));
            // Anything else on the whole text only.
            assert!(supersedes(rules, "n", "n"));
            assert!(!supersedes(rules, "m 1", "m 2"));
            // A big configstring's chunks never.
            assert!(!supersedes(rules, "x 20 abc", "x 20 abc"));
        }
        // CoD2 and CoD4 add `C` and `D`.
        assert!(supersedes(&COD2, "C 1", "C 2"));
        assert!(!supersedes(&COD1, "C 1", "C 2"));
    }
}
