//! `SV_AddServerCommand` on one client's 128-command ring, as CoD2 1.3
//! (cod2_lnxded 0x8094750, cod2-protocol.md 3.4) and CoD4 1.7 (0x81769b2)
//! run it: the same thresholds, squash and jump table in both. Each game
//! reads a command's type off its letter, which differs between them.
//! CoD1's 64-slot ring with its own thresholds is `Client::queue_server_command`.

use super::slot::Link;

/// Queues `text` on `link`'s ring; `kept` holds each slot's type (true for
/// type 1, `SV_CMD_RELIABLE`; false for type 0, `SV_CMD_CAN_IGNORE`).
/// - A client not `CS_ACTIVE`, or more than 63 commands past its ack
///   (CoD2 0x809477d, CoD4 0x81769df), first loses its unsent type 0
///   commands (CoD2 0x8094660), and a type 0 command is then not queued.
/// - A command takes the place of the first unsent type 1 one it
///   [`supersedes`]: that one leaves the ring and the new one goes last.
///
/// False when the ring would hold more than 128 unacked commands
/// (retail's `== 0x81` test, CoD2 0x8094874); the caller drops the client.
pub fn add<F>(
    link: &mut Link<F>,
    kept: &mut [bool],
    active: bool,
    text: &str,
    reliable: bool,
) -> bool {
    let pending = link.netchan.reliable_sequence as i32 - link.reliable_ack;
    if !active || pending > 63 {
        edit_unsent(link, kept, |cmds| cmds.retain(|(_, kept)| *kept));
        if !reliable {
            return true;
        }
    }
    let ring = kept.len();
    let stale = unsent(link)
        .map(|seq| slot(seq, ring))
        .position(|s| kept[s] && supersedes(text, &link.netchan.reliable[s]));
    if let Some(i) = stale {
        edit_unsent(link, kept, |cmds| drop(cmds.remove(i)));
    }
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

/// Whether `new` replaces the unsent type 1 `old` (`SV_FindPendingCommand`,
/// CoD2 0x8094530, CoD4 0x8176a76..0x8176bff): one letter, not a
/// configstring chunk `x`..`z` (CoD2 0x80945ab, CoD4 0x8176abe), and then
/// the same text after it (CoD2 0x80945dc, CoD4 0x8176adb), or a letter any
/// later one replaces (`C D a b o p q r t`, the jump tables at CoD2
/// 0x814b3c0 and CoD4 0x82326a8), or a `d`/`v` naming the same first word
/// (CoD2 0x80944aa, CoD4 0x8176b85).
pub fn supersedes(new: &str, old: &str) -> bool {
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
        || match letter {
            b'C' | b'D' | b'a' | b'b' | b'o' | b'p' | b'q' | b'r' | b't' => true,
            b'd' | b'v' => first_word(new) == first_word(old),
            _ => false,
        }
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
