//! The `cp` command a CoD2 client owes a pure server after each gamestate
//! (docs/research/cod2-protocol.md 11): without it the first `clc_move`
//! drops the client with `EXE_CANNOTVALIDATEPURECLIENT`.

use vcod_common::net::info_value_for_key;
pub use vcod_common::pak_checksum::{iwd_cp_command as cp_command, verify_iwd_cp as verify_cp};
use vcod_common::pk3::Pk3Fs;

/// `sv_iwds`: the unkeyed checksums of the server's non-localized iwds,
/// `None` when the server is not pure (the list is empty or absent).
pub fn loaded_iwds(systeminfo: &str) -> Option<Vec<i32>> {
    let sums: Vec<i32> = info_value_for_key(systeminfo, "sv_iwds")?
        .split_whitespace()
        .map(|t| t.parse::<i64>().map_or(0, |n| n as i32))
        .collect();
    (!sums.is_empty()).then_some(sums)
}

/// The `cp` to queue as a reliable command once a gamestate is parsed, after
/// the level's files are read (retail sends it after `CL_InitCGame`):
/// `fs`'s referenced non-localized iwds keyed with the gamestate's
/// `checksum_feed`. Against a pure server (`sv_iwds` in configstring 1's
/// `systeminfo`) only the iwds it lists count: the server refuses a sum it
/// did not load (11.4). Retail sends one to every server; an unpure one ignores it.
pub fn pure_command(fs: &Pk3Fs, checksum_feed: i32, systeminfo: &str) -> String {
    let allowed = loaded_iwds(systeminfo);
    let sums: Vec<i32> = fs
        .referenced_paks()
        .into_iter()
        .filter(|p| !p.localized)
        .filter(|p| allowed.as_ref().is_none_or(|a| a.contains(&p.checksum)))
        .map(|p| p.pure_checksum(checksum_feed))
        .collect();
    cp_command(&sums, checksum_feed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp_round_trips_through_the_server_check() {
        let feed = 0x1234_5678;
        let cp = cp_command(&[-5, 77], feed);
        assert_eq!(cp, format!("cp @ -5 77 {} ", feed ^ -5 ^ 77 ^ 2));
        let args: Vec<&str> = cp.split_whitespace().collect();
        assert!(verify_cp(&args, &[77, -5, 9], feed));
        assert!(!verify_cp(&args, &[77], feed));
        assert!(!verify_cp(&args, &[77, -5], feed ^ 1));
        // Nothing referenced still passes: the key alone.
        let bare = cp_command(&[], feed);
        assert_eq!(bare, format!("cp @ {feed} "));
        let args: Vec<&str> = bare.split_whitespace().collect();
        assert!(verify_cp(&args, &[], feed));
        // A repeated sum fails.
        let dup = cp_command(&[77, 77], feed);
        let args: Vec<&str> = dup.split_whitespace().collect();
        assert!(!verify_cp(&args, &[77], feed));
    }

    #[test]
    fn sv_iwds_reads_signed_sums() {
        let si = "\\sv_iwds\\-12 34 \\sv_pure\\1";
        assert_eq!(loaded_iwds(si), Some(vec![-12, 34]));
        assert_eq!(loaded_iwds("\\sv_iwds\\\\sv_pure\\0"), None);
        assert_eq!(loaded_iwds("\\sv_pure\\0"), None);
    }
}
