//! The C library's string readers as the retail binaries link them, and
//! the engine's own small string helpers every crate shares.

/// `atoi` as glibc and Darwin libc read it on i386, `(int)strtol(s, 0,
/// 10)`: leading blanks, a sign, then digits up to the first non-digit, 0
/// with none; past `i32`'s range it saturates. The Linux servers' reading
/// (1.1d, 1.5b, UO 1.51b, CoD2 lnxded) and the CoD2 Mac client's.
pub fn atoi(s: &str) -> i32 {
    let (neg, digits) = sign(s);
    let mut n: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        n = (n * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let n = if neg { -n } else { n };
    n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// `atol` as the Windows CoD1 modules link it (Visual Studio 2003 CRT:
/// CoDMP.exe 0x005253bd, cgame_mp_x86.dll 0x3004af56, ui_mp_x86.dll
/// 0x40019b68): the same prefix as [`atoi`], but the digits accumulate in
/// an `int` with no overflow check, so a long number wraps.
pub fn atoi_msvc(s: &str) -> i32 {
    let (neg, digits) = sign(s);
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i32, |n, d| {
            n.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    if neg { n.wrapping_neg() } else { n }
}

/// Bytes as a Latin-1 `String`, one char per byte, so nothing is lost
/// and a byte's glyph index survives the round trip.
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// `Q_CleanStr` (CoD2's `I_CleanStr`): colour codes out (`^` and the
/// character after it, unless that is another `^`), then anything outside
/// printable ASCII.
pub fn clean_str(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|&n| n != '^') {
            chars.next();
        } else if (' '..='~').contains(&c) {
            out.push(c);
        }
    }
    out
}

/// The text after leading blanks and one sign, and whether that sign was `-`.
fn sign(s: &str) -> (bool, &str) {
    let s = s.trim_start();
    match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_reads_the_c_prefix() {
        for f in [atoi, atoi_msvc] {
            assert_eq!(f("  -42.9x"), -42);
            assert_eq!(f("+7"), 7);
            assert_eq!(f("12abc"), 12);
            assert_eq!(f("x1"), 0);
            assert_eq!(f("-"), 0);
            assert_eq!(f("+-5"), 0);
            assert_eq!(f(""), 0);
        }
    }

    #[test]
    fn strtol_saturates_where_msvc_wraps() {
        assert_eq!(atoi("2147483648"), i32::MAX);
        assert_eq!(atoi("-2147483649"), i32::MIN);
        assert_eq!(atoi("99999999999999999999"), i32::MAX);
        assert_eq!(atoi_msvc("2147483648"), i32::MIN);
        assert_eq!(atoi_msvc("-2147483648"), i32::MIN);
        assert_eq!(atoi_msvc("4294967297"), 1);
    }
}
