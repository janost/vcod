//! The window's video mode from `r_mode`, `r_fullscreen` and the custom
//! size, and the pick of a monitor mode for exclusive fullscreen
//! (docs/research/cod11-front-end.md, "vcod: fullscreen and wide windows").

/// CoDMP.exe's mode table (0x5716c0, 13 rows of name, width, height and
/// pixel aspect 1.0, read by `R_GetModeInfo` 0x4b0ec0).
pub const MODES: [(u32, u32); 13] = [
    (320, 240),
    (400, 300),
    (512, 384),
    (640, 480),
    (800, 600),
    (960, 720),
    (1024, 768),
    (1152, 864),
    (1280, 1024),
    (1600, 1200),
    (2048, 1536),
    (856, 480),
    (1920, 1200),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fullscreen {
    Windowed,
    /// `r_fullscreen 1`: a borderless window over the whole monitor at the
    /// desktop's own resolution.
    Borderless,
    /// `r_fullscreen 2`: the monitor switched to the mode closest to the
    /// asked size.
    Exclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    /// The size `r_mode` asks for; `None` keeps the window's own.
    pub size: Option<(u32, u32)>,
    pub fullscreen: Fullscreen,
    /// The refresh rate exclusive fullscreen asks for (CoD2's and CoD4's
    /// `r_displayRefresh`, `%i Hz`); `None` takes the highest.
    pub refresh_hz: Option<u32>,
}

/// What the cvars ask for. `r_mode` -1 is the custom size, as in
/// `R_GetModeInfo`, when both its sides are set; vcod registers them 0, its
/// own window size. Any other mode past the table keeps the window's size.
/// CoD2's `r_mode` is the mode's name, `WxH` ([`cod2_mode_lists`]).
pub fn video_mode(r_mode: &str, r_fullscreen: i32, custom: (i32, i32)) -> VideoMode {
    let named = parse_size(r_mode);
    let index = r_mode.trim().parse::<f32>().map_or(0, |m| m as i32);
    let size = match (named, index) {
        (Some(size), _) => Some(size),
        (None, -1) if custom.0 > 0 && custom.1 > 0 => Some((custom.0 as u32, custom.1 as u32)),
        (None, m) => usize::try_from(m).ok().and_then(|m| MODES.get(m).copied()),
    };
    let fullscreen = match r_fullscreen {
        0 => Fullscreen::Windowed,
        2 => Fullscreen::Exclusive,
        _ => Fullscreen::Borderless,
    };
    VideoMode {
        size,
        fullscreen,
        refresh_hz: None,
    }
}

/// `WxH` as a size, both sides parsed as integers.
pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// The sizes [`cod2_mode_lists`] lists under a display that reports only
/// its own: the usual 4:3, 16:10 and 16:9 desktop modes.
const STANDARD_SIZES: [(u32, u32); 18] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1152, 864),
    (1280, 720),
    (1280, 800),
    (1280, 960),
    (1280, 1024),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1600, 1200),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1080),
    (2560, 1440),
    (2560, 1600),
];

/// One of a monitor's video modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorMode {
    pub size: (u32, u32),
    pub refresh_mhz: u32,
    pub bit_depth: u16,
}

/// CoD2's `r_mode` and `r_displayRefresh` domains off the display's modes
/// (`R_BeginRegistration`, Mac 0xcb5f6): the modes sorted by width, height
/// and refresh; each size of at least 640 x 480 once, as `%ix%i`; each
/// refresh rate once in that order, as `%i Hz`.
pub fn cod2_mode_lists(modes: &[MonitorMode]) -> (Vec<String>, Vec<String>) {
    let hz = |m: &MonitorMode| (m.refresh_mhz + 500) / 1000;
    let mut sorted: Vec<(u32, u32, u32)> =
        modes.iter().map(|m| (m.size.0, m.size.1, hz(m))).collect();
    sorted.sort_unstable();
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    let mut rates: Vec<u32> = Vec::new();
    for &(w, h, r) in &sorted {
        if w >= 640 && h >= 480 && sizes.last() != Some(&(w, h)) {
            sizes.push((w, h));
        }
        if !rates.contains(&r) {
            rates.push(r);
        }
    }
    // A Wayland output names only the mode in use, where retail's adapter
    // lists every mode it drives: the common sizes that fit it stand in.
    if let [native] = sizes[..] {
        sizes = STANDARD_SIZES
            .iter()
            .copied()
            .filter(|&(w, h)| w <= native.0 && h <= native.1 && (w, h) != native)
            .chain([native])
            .collect();
        sizes.sort_unstable();
    }
    (
        sizes.iter().map(|(w, h)| format!("{w}x{h}")).collect(),
        rates.iter().map(|r| format!("{r} Hz")).collect(),
    )
}

/// The index in `modes` of the one closest to `want`: the same size if
/// the monitor has it, else the least difference in area, then in aspect;
/// among equals the highest refresh rate, then the deepest colour.
/// Among equal sizes the refresh rate nearest `hz` comes first when one is
/// asked for.
pub fn closest_mode_at(want: (u32, u32), hz: Option<u32>, modes: &[MonitorMode]) -> Option<usize> {
    let off_hz = |m: &MonitorMode| {
        hz.map_or(0, |hz| {
            (i64::from(m.refresh_mhz) - i64::from(hz) * 1000).abs()
        })
    };
    let area = |(w, h): (u32, u32)| i64::from(w) * i64::from(h);
    let aspect = |(w, h): (u32, u32)| w as f32 / h.max(1) as f32;
    let key = |m: &MonitorMode| {
        (
            (area(m.size) - area(want)).abs(),
            ((aspect(m.size) - aspect(want)).abs() * 1000.0) as i64,
            off_hz(m),
            std::cmp::Reverse(m.refresh_mhz),
            std::cmp::Reverse(m.bit_depth),
        )
    };
    (0..modes.len()).min_by_key(|&i| key(&modes[i]))
}

/// Alt+Enter (CoDMP.exe 0x466e52, `WM_SYSKEYDOWN` with `VK_RETURN`): the
/// `r_fullscreen` it sets, `!r_fullscreen`, before it queues `vid_restart`.
/// Retail skips it while a map loads (`cls.state` 4).
pub fn toggled(r_fullscreen: i32) -> i32 {
    i32::from(r_fullscreen == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_mode_reads_the_table_and_minus_one_the_custom_size() {
        assert_eq!(video_mode("3", 0, (0, 0)).size, Some((640, 480)));
        assert_eq!(video_mode("12", 0, (0, 0)).size, Some((1920, 1200)));
        assert_eq!(video_mode("13", 0, (0, 0)).size, None);
        assert_eq!(video_mode("-1", 0, (0, 0)).size, None, "the window's own");
        assert_eq!(video_mode("-1", 0, (3440, 1440)).size, Some((3440, 1440)));
        assert_eq!(video_mode("-1", 0, (2560, 0)).size, None);
        assert_eq!(video_mode("-2", 0, (2560, 1080)).size, None);
    }

    #[test]
    fn r_fullscreen_picks_windowed_borderless_or_exclusive() {
        let full = |v| video_mode("-1", v, (0, 0)).fullscreen;
        assert_eq!(full(0), Fullscreen::Windowed);
        assert_eq!(full(1), Fullscreen::Borderless);
        assert_eq!(full(2), Fullscreen::Exclusive);
        assert_eq!(full(3), Fullscreen::Borderless);
    }

    #[test]
    fn exclusive_takes_the_exact_size_at_its_best_rate_else_the_nearest() {
        let m = |w, h, hz: u32, bits| MonitorMode {
            size: (w, h),
            refresh_mhz: hz * 1000,
            bit_depth: bits,
        };
        let modes = [
            m(1920, 1080, 60, 32),
            m(2560, 1440, 60, 32),
            m(2560, 1440, 144, 32),
            m(2560, 1440, 144, 24),
            m(1280, 720, 60, 32),
            m(1600, 900, 60, 32),
            m(3440, 1440, 100, 32),
        ];
        assert_eq!(closest_mode_at((2560, 1440), None, &modes), Some(2));
        assert_eq!(closest_mode_at((3440, 1440), None, &modes), Some(6));
        // 1600x1024 is nearest 1600x900 by area.
        assert_eq!(closest_mode_at((1600, 1024), None, &modes), Some(5));
        // 1920x1200 (16:10) has 1920x1080 nearest.
        assert_eq!(closest_mode_at((1920, 1200), None, &modes), Some(0));
        assert_eq!(closest_mode_at((640, 480), None, &[]), None);
    }

    /// `r_displayRefresh` picks among the asked size's rates.
    #[test]
    fn a_refresh_rate_picks_among_equal_sizes() {
        let m = |w, h, hz: u32| MonitorMode {
            size: (w, h),
            refresh_mhz: hz * 1000,
            bit_depth: 32,
        };
        let modes = [m(1920, 1080, 60), m(1920, 1080, 144), m(1280, 720, 60)];
        assert_eq!(closest_mode_at((1920, 1080), None, &modes), Some(1));
        assert_eq!(closest_mode_at((1920, 1080), Some(60), &modes), Some(0));
        assert_eq!(closest_mode_at((1280, 720), Some(144), &modes), Some(2));
    }

    #[test]
    fn alt_enter_flips_r_fullscreen_as_a_boolean() {
        assert_eq!(toggled(0), 1);
        assert_eq!(toggled(1), 0);
        assert_eq!(toggled(2), 0);
    }

    #[test]
    fn cod2_lists_each_size_from_640_by_480_and_each_rate() {
        let m = |w, h, hz: u32| MonitorMode {
            size: (w, h),
            refresh_mhz: hz * 1000 - 40,
            bit_depth: 32,
        };
        let modes = [
            m(1920, 1080, 60),
            m(800, 600, 75),
            m(640, 400, 70),
            m(1920, 1080, 144),
            m(800, 600, 60),
        ];
        let (sizes, rates) = cod2_mode_lists(&modes);
        assert_eq!(sizes, ["800x600", "1920x1080"]);
        assert_eq!(rates, ["70 Hz", "60 Hz", "75 Hz", "144 Hz"]);
        assert_eq!(video_mode("800x600", 0, (0, 0)).size, Some((800, 600)));
    }

    /// A display that reports only its own mode (a Wayland output) lists
    /// the standard sizes that fit inside it too.
    #[test]
    fn a_lone_mode_lists_the_standard_sizes_under_it() {
        let modes = [MonitorMode {
            size: (1920, 1080),
            refresh_mhz: 59_940,
            bit_depth: 32,
        }];
        let (sizes, rates) = cod2_mode_lists(&modes);
        assert_eq!(sizes.first().map(String::as_str), Some("640x480"));
        assert_eq!(sizes.last().map(String::as_str), Some("1920x1080"));
        assert!(sizes.contains(&"1280x720".to_string()));
        assert!(!sizes.iter().any(|s| s == "1920x1200" || s == "2560x1440"));
        assert_eq!(rates, ["60 Hz"]);
    }
}
