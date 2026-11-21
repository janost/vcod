//! The C library's string readers as the retail binaries link them, and
//! the engine's own small string helpers every crate shares.

/// Runs `f` on `s` with ASCII folded to lowercase, on the stack when it
/// fits in 64 bytes: a by-name lookup in a table keyed by folded names
/// allocates nothing.
pub fn with_ascii_lowercase<R>(s: &str, f: impl FnOnce(&str) -> R) -> R {
    if !s.bytes().any(|c| c.is_ascii_uppercase()) {
        return f(s);
    }
    let mut buf = [0u8; 64];
    match buf.get_mut(..s.len()) {
        Some(dst) => {
            dst.copy_from_slice(s.as_bytes());
            dst.make_ascii_lowercase();
            f(std::str::from_utf8(dst).expect("ASCII folding keeps UTF-8 valid"))
        }
        None => f(&s.to_ascii_lowercase()),
    }
}

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

/// A text file's bytes: UTF-8 when they are, else [`latin1`], so a file
/// in the engine's byte strings (a retail config's `ö`) loses nothing.
pub fn utf8_or_latin1(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| latin1(e.as_bytes()))
}

/// [`latin1`]'s inverse: one byte per char, a char above U+00FF as `.`, as
/// `write_big_string` sends one. Its low byte could be a NUL or a `\`.
pub fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'.')).collect()
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

/// Text bound for a terminal or a log file with every control character
/// but newline and tab (C0, DEL, C1) shown as `\xNN`, so a peer's string
/// cannot move the cursor, clear the screen or start a forged line with CR.
pub fn escape_controls(s: &str) -> std::borrow::Cow<'_, str> {
    let bad = |c: char| c.is_control() && c != '\n' && c != '\t';
    if !s.chars().any(bad) {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if bad(c) {
            out.push_str(&format!("\\x{:02x}", c as u32));
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// C's `atof` over the decimal forms the games' files and commands use:
/// leading blanks, a sign, digits with at most one point, then an exponent
/// only when digits follow it; the longest such prefix, 0 with none. The
/// result is the double `atof` returns; a caller that stores a float casts
/// it, as the binaries do. `inf`, `nan` and hex forms read as 0.
pub fn atof(s: &str) -> f64 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    if matches!(b.first(), Some(b'-' | b'+')) {
        end = 1;
    }
    let digits = |end: &mut usize| {
        while b.get(*end).is_some_and(u8::is_ascii_digit) {
            *end += 1;
        }
    };
    digits(&mut end);
    if b.get(end) == Some(&b'.') {
        end += 1;
        digits(&mut end);
    }
    let mut best = t[..end].parse::<f64>().unwrap_or(0.0);
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let mut e = end + 1;
        if matches!(b.get(e), Some(b'-' | b'+')) {
            e += 1;
        }
        let start = e;
        digits(&mut e);
        if e > start
            && let Ok(v) = t[..e].parse::<f64>()
        {
            best = v;
        }
    }
    best
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
    fn atof_takes_the_c_prefix() {
        assert_eq!(atof(" 1.5e2x"), 150.0);
        assert_eq!(atof(".25"), 0.25);
        assert_eq!(atof("-.5"), -0.5);
        assert_eq!(atof("1.5.3"), 1.5);
        for zero in ["", ".", "-.", "+", "x1", "inf"] {
            assert_eq!(atof(zero), 0.0, "{zero:?}");
        }
        // An exponent with no digits is not read.
        assert_eq!(atof("3e"), 3.0);
        assert_eq!(atof("1e+"), 1.0);
        assert_eq!(atof("2E-1x"), 0.2);
    }

    #[test]
    fn escape_controls_shows_c0_del_and_c1_and_keeps_newline_and_tab() {
        assert_eq!(
            escape_controls("a\x1b[2J\rb\x7f\u{9b}c\n\td"),
            "a\\x1b[2J\\x0db\\x7f\\x9bc\n\td"
        );
        assert!(matches!(
            escape_controls("plain é"),
            std::borrow::Cow::Borrowed("plain é")
        ));
    }

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
