//! The `scriptUsage` and `stringUsage` console commands (cod_lnxded
//! 0x809d2d0 and 0x809a780): retail's output over vcod's own VM, whose
//! threads, objects and strings are not retail's variable and memory trees.
//! docs/research/cod11-server-handshake.md, "Console commands over rcon".

use vcod_gsc::ThreadUsage;

const RULE: &str = "********************************\n";

/// `MT_InitBits`' tree: 65536 eight-byte buckets, bucket 0 never handed out.
const BUCKETS: usize = 0xffff;

/// Threads grouped by their whole call stack, sorted as retail's qsort
/// orders code addresses (file install order, then line; a shorter stack
/// first on a shared prefix), each group's count and its var and endon
/// usage summed as floats and truncated (0x809d556..0x809d5b3) over the
/// innermost position and its callers, then `(class, count, fields)` for
/// every object class that has any.
pub fn script_usage(threads: &[ThreadUsage], classes: &[(&str, usize, usize)]) -> String {
    let mut threads = threads.to_vec();
    threads.sort_by(|a, b| a.stack.cmp(&b.stack));
    let mut out = String::from(RULE);
    for group in threads.chunk_by(|a, b| a.stack == b.stack) {
        let vars = group.iter().fold(0.0f32, |s, t| s + t.vars) as i32;
        let endons = group.iter().fold(0.0f32, |s, t| s + t.endons) as i32;
        out.push_str(&format!(
            "count: {}, var usage: {vars}, endon usage: {endons}\n",
            group.len()
        ));
        for (i, (_, file, line)) in group[0].stack.iter().enumerate() {
            if i > 0 {
                out.push_str("called from:\n");
            }
            out.push_str(&format!(
                "(file '{}.gsc', line {line})\n",
                file.replace('/', "\\")
            ));
        }
    }
    out.push_str(RULE);
    for (class, count, vars) in classes.iter().filter(|c| c.1 > 0) {
        out.push_str(&format!(
            "ent type '{class}'... count: {count}, var usage: {vars}\n"
        ));
    }
    out.push_str(RULE);
    out
}

/// Each string as `SL_GetString` would size it in the tree: a four-byte
/// header and the NUL, rounded up to a power of two of buckets
/// (`MT_GetSize` 0x809ae08). The free space is reported as one block per
/// set bit, the tree a compacted heap would leave.
pub fn string_usage(lengths: &[usize]) -> String {
    let used: usize = lengths
        .iter()
        .map(|l| (l + 5).div_ceil(8).next_power_of_two())
        .sum();
    let free = BUCKETS.saturating_sub(used);
    let mut out = String::from(RULE);
    for n in 0..=16 {
        let blocks = (free >> n) & 1;
        out.push_str(&format!(
            "{n} subtree has {blocks} * {} = {} free buckets\n",
            1 << n,
            blocks << n
        ));
    }
    out.push_str(RULE);
    out.push_str(RULE);
    out.push_str(&format!(
        "total memory alloc buckets: {used} ({} instances)\n",
        lengths.len()
    ));
    out.push_str(&format!("total memory free buckets: {free}\n"));
    out.push_str(RULE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(stack: &[(u32, &str, u32)], vars: f32, endons: f32) -> ThreadUsage {
        ThreadUsage {
            stack: stack
                .iter()
                .map(|(o, f, l)| (*o, f.to_string(), *l))
                .collect(),
            vars,
            endons,
        }
    }

    /// Retail's shape (captured on 1.1d): one block per distinct stack, the
    /// waiting position first and every caller after a `called from:`.
    #[test]
    fn script_usage_groups_threads_by_stack() {
        let a = [
            (0, "maps/mp/gametypes/dm", 40),
            (0, "maps/mp/gametypes/dm", 12),
        ];
        let text = script_usage(
            &[
                thread(&a, 3.0, 1.0),
                thread(&[(0, "maps/mp/_load", 7)], 2.0, 0.0),
                thread(&a, 4.0, 1.0),
            ],
            &[("entity", 211, 431), ("hudelem", 1, 1), ("pathnode", 0, 0)],
        );
        assert_eq!(
            text,
            "********************************\n\
             count: 1, var usage: 2, endon usage: 0\n\
             (file 'maps\\mp\\_load.gsc', line 7)\n\
             count: 2, var usage: 7, endon usage: 2\n\
             (file 'maps\\mp\\gametypes\\dm.gsc', line 40)\n\
             called from:\n\
             (file 'maps\\mp\\gametypes\\dm.gsc', line 12)\n\
             ********************************\n\
             ent type 'entity'... count: 211, var usage: 431\n\
             ent type 'hudelem'... count: 1, var usage: 1\n\
             ********************************\n"
        );
    }

    /// Retail 1.1d on mp_brecourt, `tdm`, with `developer_script 1`: the
    /// gametype's two waits, then the minefields' 32 (handshake doc,
    /// "Console commands over rcon"). The gametype installs first, so it
    /// sorts first although `_minefields` sorts before it by name.
    #[test]
    fn script_usage_orders_groups_by_install_order() {
        let tdm = "maps/mp/gametypes/tdm";
        let mines = [(7, "maps/mp/_minefields", 19)];
        let mut threads = vec![thread(&mines, 2.0, 0.0); 32];
        threads.push(thread(&[(0, tdm, 1184)], 6.0, 0.0));
        threads.push(thread(&[(0, tdm, 992)], 2.0, 0.0));
        let text = script_usage(&threads, &[]);
        let blocks: Vec<&str> = text.lines().filter(|l| l.starts_with("(file")).collect();
        assert_eq!(
            blocks,
            [
                "(file 'maps\\mp\\gametypes\\tdm.gsc', line 992)",
                "(file 'maps\\mp\\gametypes\\tdm.gsc', line 1184)",
                "(file 'maps\\mp\\_minefields.gsc', line 19)",
            ]
        );
        assert!(text.contains("count: 32, var usage: 64, endon usage: 0\n"));
    }

    #[test]
    fn string_usage_sizes_strings_in_power_of_two_buckets() {
        // 3 + 5 bytes is one bucket, 4 + 5 two, 12 + 5 three, rounded to four.
        let text = string_usage(&[3, 4, 12]);
        assert!(text.contains("total memory alloc buckets: 7 (3 instances)\n"));
        assert!(text.contains("total memory free buckets: 65528\n"));
        // 65528 = 0b1111_1111_1111_1000.
        assert!(text.contains("2 subtree has 0 * 4 = 0 free buckets\n"));
        assert!(text.contains("3 subtree has 1 * 8 = 8 free buckets\n"));
        assert!(text.contains("16 subtree has 0 * 65536 = 0 free buckets\n"));
        assert_eq!(text.lines().count(), 23);
    }
}
