//! The `cp` command a CoD4 1.7 client owes a pure server after each
//! gamestate (docs/research/cod4-handshake.md, "The pure check"). Without it
//! the first move after the client enters the world drops it with
//! `EXE_CANNOTVALIDATEPURECLIENT`.

use anyhow::{Context, Result};
use std::path::Path;
use vcod_common::net::info_value_for_key;
use vcod_common::pak_checksum::entry_crcs;

/// `Com_BlockChecksumKey32`: the reflected CRC-32 (polynomial 0xedb88320)
/// seeded with `!key` and inverted at the end, so key 0 is plain CRC-32.
pub fn block_checksum_key32(data: &[u8], key: u32) -> u32 {
    let mut crc = !key;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// An iwd's checksum keyed with `checksum_feed`: the CRC-32s of its
/// non-empty entries as little-endian words, through
/// [`block_checksum_key32`]. Feed 0 gives the unkeyed checksum `sv_iwds`
/// lists; the gamestate's feed gives the pure checksum `cp` carries.
pub fn iwd_checksum(crcs: &[u32], checksum_feed: i32) -> i32 {
    let words: Vec<u8> = crcs.iter().flat_map(|c| c.to_le_bytes()).collect();
    block_checksum_key32(&words, checksum_feed as u32) as i32
}

/// `CL_SendPureChecksums` (iw3mp.exe 0x46a0f0) over
/// `FS_ReferencedIwdPureChecksums`: `cp @`, each pure checksum, then
/// `checksumFeed` XOR all of them XOR their count, every number `"%i "`.
pub fn cp_command(sums: &[i32], checksum_feed: i32) -> String {
    let mut s = String::from("cp @ ");
    let mut key = checksum_feed;
    for &c in sums {
        s.push_str(&format!("{c} "));
        key ^= c;
    }
    s.push_str(&format!("{} ", key ^ sums.len() as i32));
    s
}

/// `sv_iwds`: the unkeyed checksums of the server's non-localized iwds.
pub fn loaded_iwds(systeminfo: &str) -> Vec<i32> {
    info_value_for_key(systeminfo, "sv_iwds")
        .unwrap_or("")
        .split_whitespace()
        .map(|t| t.parse::<i64>().map_or(0, |n| n as i32))
        .collect()
}

/// The `cp` for a gamestate: the pure checksum of every `*.iwd` in `main`
/// whose unkeyed checksum the server lists in `sv_iwds`, in that list's
/// order. With no match it is the bare key, which the server's check also
/// passes.
pub fn pure_command(main: &Path, systeminfo: &str, checksum_feed: i32) -> Result<String> {
    let listed = loaded_iwds(systeminfo);
    let mut ours = Vec::new();
    for entry in std::fs::read_dir(main).with_context(|| format!("{}", main.display()))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("iwd"))
        {
            let f = std::fs::File::open(&path)?;
            let crcs = entry_crcs(std::io::BufReader::new(f))
                .with_context(|| format!("{}", path.display()))?;
            ours.push((iwd_checksum(&crcs, 0), iwd_checksum(&crcs, checksum_feed)));
        }
    }
    let sums: Vec<i32> = listed
        .iter()
        .filter_map(|l| ours.iter().find(|(c, _)| c == l).map(|&(_, p)| p))
        .collect();
    Ok(cp_command(&sums, checksum_feed))
}

/// `SV_VerifyIwds_f` (cod4_lnxded 0x817059c) on a `cp` command's arguments
/// (`args[0]` is `cp`): `@`, sums that are distinct and each among the
/// server's `loaded` pure checksums, then the key. True is retail's
/// `pureAuthentic` 1.
pub fn verify_cp(args: &[&str], loaded: &[i32], checksum_feed: i32) -> bool {
    if args.len() < 3 || !args[1].starts_with('@') {
        return false;
    }
    let tail: Vec<i32> = args[2..]
        .iter()
        .map(|a| a.parse::<i64>().map_or(0, |v| v as i32))
        .collect();
    let (sums, key) = tail.split_at(tail.len() - 1);
    for (i, s) in sums.iter().enumerate() {
        if sums[..i].contains(s) || !loaded.contains(s) {
            return false;
        }
    }
    let k = sums.iter().fold(checksum_feed, |k, s| k ^ s);
    (k ^ sums.len() as i32) == key[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key32_is_crc32() {
        assert_eq!(block_checksum_key32(b"123456789", 0), 0xcbf4_3926);
    }

    #[test]
    fn cp_passes_the_server_check() {
        let feed = 995670164;
        let cp = cp_command(&[-5, 77], feed);
        assert_eq!(cp, format!("cp @ -5 77 {} ", feed ^ -5 ^ 77 ^ 2));
        let args: Vec<&str> = cp.split_whitespace().collect();
        assert!(verify_cp(&args, &[77, -5, 9], feed));
        assert!(!verify_cp(&args, &[77], feed));
        assert!(!verify_cp(&args, &[77, -5], feed ^ 1));
        let bare = cp_command(&[], feed);
        assert_eq!(bare, format!("cp @ {feed} "));
        let args: Vec<&str> = bare.split_whitespace().collect();
        assert!(verify_cp(&args, &[], feed));
    }
}
