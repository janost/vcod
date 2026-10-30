//! The `cp` sums against a retail 1.3 server's own lists
//! (docs/research/cod2-protocol.md 11).

use vcod_cod2::net::pure;

/// `sv_iwds` and `sv_iwdNames` of a stock 1.3 `cod2_lnxded` on mp_toujane,
/// read off its systeminfo on 2026-10-22.
const SV_IWDS: &str = "181429573 -1449716526 780394069 -1333623355 -1980843666 1334775335 \
     -621896007 1101180720 1046874969 1053665859 1842349204 -1652414412 1659111092 \
     -1085686032 -2025394354 178615151";
const SV_IWD_NAMES: &str = "iw_15 iw_14 iw_13 iw_12 iw_11 iw_10 iw_09 iw_08 iw_07 iw_06 iw_05 iw_04 iw_03 iw_02 iw_01 iw_00";

#[test]
fn iwd_checksums_match_the_servers_sv_iwds() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let ours: Vec<(String, i32)> = fs
        .paks()
        .filter(|p| !p.localized)
        .map(|p| (p.name.clone(), p.checksum))
        .collect();
    let retail: Vec<(String, i32)> = SV_IWD_NAMES
        .split(' ')
        .map(str::to_string)
        .zip(SV_IWDS.split_whitespace().map(|s| s.parse().unwrap()))
        .collect();
    assert_eq!(ours, retail);
}

#[test]
fn the_maps_iwd_keys_to_the_sum_retail_took() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    // The live session's feed, and the `cp` the server accepted with it.
    let feed = 494_263_159;
    fs.clear_references();
    fs.touch("maps/mp/mp_toujane.d3dbsp");
    let si = format!("\\sv_iwds\\{SV_IWDS} \\sv_pure\\1");
    assert_eq!(
        pure::pure_command(&fs, feed, &si),
        "cp @ -1059352459 -575780093 "
    );
    // Nothing read: the bare key.
    fs.clear_references();
    assert_eq!(pure::pure_command(&fs, feed, &si), format!("cp @ {feed} "));
}
