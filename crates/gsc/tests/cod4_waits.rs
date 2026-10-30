//! CoD4's frame-counted `wait` through `VmProfile::COD4`, against the
//! lines retail 1.7 logged for the same waits (docs/research/cod4-gsc.md
//! 4.1). The probe is C4-55's mover probe with the mover reads cut: each
//! thread logs `getTime()` past its start after a fractional wait.

use vcod_gsc::{
    Atom, Cx, Dialect, EntId, ErrorKind, Host, Loader, ScriptSource, Target, Value, Vm, VmProfile,
};

const PATH: &str = "maps/mp/probe_waits";

const PROBE: &str = r#"
main()
{
	level thread waitProbe();
}

waitProbe()
{
	wait 2;
	t0 = getTime();
	thread readAfter( t0, 0.6 );
	thread readAfter( t0, 0.643 );
	thread readAfter( t0, 0.65 );
	thread readAfter( t0, 0.62 );
	thread readAfter( t0, 0.624 );
	thread readAfter( t0, 0.626 );
	thread readAfter( t0, 0.6001 );
	thread readAfter( t0, 0.649 );
	thread readAfter( t0, 0.651 );
	thread readAfter( t0, 0.01 );
	thread readAfter( t0, 0.024 );
	thread readAfter( t0, 0.026 );
	thread bombLike();
	thread waitZero( t0 );
	for ( i = 0; i < 6; i++ )
	{
		wait 0.05;
		logPrint( "PROBE plane frame " + ( getTime() - t0 ) + "\n" );
	}
	wait 3;
	logPrint( "PROBE done\n" );
}

readAfter( t0, secs )
{
	wait secs;
	logPrint( "PROBE plane wait " + secs + " at " + ( getTime() - t0 ) + "\n" );
}

bombLike()
{
	t1 = getTime();
	wait 0.85;
	logPrint( "PROBE bomb 0.85 at " + ( getTime() - t1 ) + "\n" );
	wait 0.15;
	logPrint( "PROBE bomb 1.0 at " + ( getTime() - t1 ) + "\n" );
	wait 0.05;
	logPrint( "PROBE bomb 1.05 at " + ( getTime() - t1 ) + "\n" );
}

waitZero( t0 )
{
	logPrint( "PROBE zero before " + ( getTime() - t0 ) + "\n" );
	wait 0;
	logPrint( "PROBE zero after " + ( getTime() - t0 ) + "\n" );
	wait 0.0001;
	logPrint( "PROBE tiny after " + ( getTime() - t0 ) + "\n" );
	wait 1.0249;
	logPrint( "PROBE 1.0249 after " + ( getTime() - t0 ) + "\n" );
	wait 1.025;
	logPrint( "PROBE 1.025 after " + ( getTime() - t0 ) + "\n" );
}
"#;

/// Retail `cod4_lnxded` 1.7's `games_mp.log` for the mover probe on
/// mp_crash, in log order, with each line's mover read (` x ..`, ` z ..`)
/// and the start line dropped (cod4-gsc.md 4.1).
const RETAIL: &[&str] = &[
    "PROBE zero before 0",
    "PROBE zero after 0",
    "PROBE tiny after 50",
    "PROBE plane frame 50",
    "PROBE plane wait 0.026 at 50",
    "PROBE plane wait 0.024 at 50",
    "PROBE plane wait 0.01 at 50",
    "PROBE plane frame 100",
    "PROBE plane frame 150",
    "PROBE plane frame 200",
    "PROBE plane frame 250",
    "PROBE plane frame 300",
    "PROBE plane wait 0.6001 at 600",
    "PROBE plane wait 0.624 at 600",
    "PROBE plane wait 0.62 at 600",
    "PROBE plane wait 0.6 at 600",
    "PROBE plane wait 0.651 at 650",
    "PROBE plane wait 0.649 at 650",
    "PROBE plane wait 0.626 at 650",
    "PROBE plane wait 0.65 at 650",
    "PROBE plane wait 0.643 at 650",
    "PROBE bomb 0.85 at 850",
    "PROBE bomb 1.0 at 1000",
    "PROBE bomb 1.05 at 1050",
    "PROBE 1.0249 after 1050",
    "PROBE 1.025 after 2050",
    "PROBE done",
];

struct Source;

impl ScriptSource for Source {
    fn read(&self, canonical: &str) -> Option<String> {
        (canonical == PATH).then(|| PROBE.to_string())
    }
}

/// `logPrint` collects, `getTime` reads the frame's `level.time`.
#[derive(Default)]
struct WaitHost {
    now: i32,
    lines: Vec<String>,
}

impl Host for WaitHost {
    fn builtin(
        &mut self,
        cx: &mut Cx,
        name: Atom,
        _recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind> {
        match cx.resolve_folded(name) {
            "logprint" => {
                if let Some(Value::String(a)) = args.first() {
                    self.lines.extend(cx.resolve(*a).lines().map(String::from));
                }
                Ok(Value::Undefined)
            }
            "gettime" => Ok(Value::Int(self.now)),
            _ => Err(ErrorKind::MissingBuiltin(name)),
        }
    }

    fn get_field(&mut self, _cx: &mut Cx, _e: EntId, _f: Atom) -> Value {
        Value::Undefined
    }

    fn set_field(&mut self, _cx: &mut Cx, _e: EntId, _f: Atom, _v: Value) -> Result<(), ErrorKind> {
        Ok(())
    }
}

/// The probe's lines under `profile`, on 50 ms frames from 0.
fn run(profile: &VmProfile) -> Vec<String> {
    let mut vm = Vm::new();
    Loader::with_dialect(Box::new(Source), profile.dialect)
        .load(&mut vm, PATH)
        .unwrap_or_else(|e| panic!("compile: {} (line {})", e.msg, e.line));
    vm.apply_profile(profile);
    let main = vm.func_ref(PATH, "main");
    let mut host = WaitHost::default();
    vm.start_thread(&mut host, main, None, Vec::new());
    for frame in 1..=200 {
        host.now = frame * 50;
        let now = host.now;
        let errors = vm.run_frame(&mut host, now);
        assert!(errors.is_empty(), "{errors:?}");
    }
    host.lines
}

#[test]
fn cod4_waits_log_retails_frames() {
    assert_eq!(run(&VmProfile::COD4), RETAIL);
}

/// The gate above is only worth having if CoD1's millisecond waits fail it.
#[test]
fn millisecond_waits_miss_retails_frames() {
    let profile = VmProfile {
        frame_waits: false,
        ..VmProfile::COD4
    };
    assert_eq!(profile.dialect, Dialect::Cod4);
    assert_ne!(run(&profile), RETAIL);
}
