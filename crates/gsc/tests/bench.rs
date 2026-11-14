//! The interpreter's microbench, ignored: CoD2's `getSpawnpoint_DM`
//! (`maps/mp/gametypes/_spawnlogic.gsc`) cut to its loops, one call per
//! spawning player, every spawn point weighed against every other player,
//! over a host that answers the entity fields and builtins it reads from
//! plain tables (cod4-server-frame.md 9, "The spawn wave"). Run in release:
//!
//!     cargo test --release -p vcod-gsc --test gsc bench:: -- --ignored --nocapture
//!
//! It prints the wave's wall time and the result, which must not move.

use vcod_gsc::{
    Atom, Cx, Dialect, EntId, ErrorKind, Host, Loader, ScriptSource, Target, Value, Vm, VmProfile,
};

const PATH: &str = "maps/mp/bench_spawnlogic";

const SCRIPT: &str = r#"
main()
{
	level.total = 0;
	players = getentarray( "player", "classname" );
	spawnpoints = getentarray( "mp_dm_spawn", "classname" );
	for ( p = 0; p < players.size; p++ )
	{
		best = players[p] pick( spawnpoints );
		level.total += best;
	}
	return level.total;
}

abs( x )
{
	if ( x < 0 )
		x = x * -1;
	return x;
}

add_to_array( array, ent )
{
	if ( !isdefined( array ) )
		array[0] = ent;
	else
		array[array.size] = ent;
	return array;
}

others()
{
	alive = [];
	players = getentarray( "player", "classname" );
	for ( i = 0; i < players.size; i++ )
	{
		player = players[i];
		if ( player.sessionstate == "spectator" || player.sessionstate == "dead" || player == self )
			continue;
		alive = add_to_array( alive, player );
	}
	return alive;
}

pick( spawnpoints )
{
	weights = [];
	for ( i = 0; i < spawnpoints.size; i++ )
		weights[i] = 0;
	alive = others();
	idealDist = 1600;
	badDist = 1200;
	for ( i = 0; i < spawnpoints.size; i++ )
	{
		totalDistFromIdeal = 0;
		nearbyBadAmount = 0;
		for ( j = 0; j < alive.size; j++ )
		{
			dist = distance( spawnpoints[i].origin, alive[j].origin );
			if ( dist < badDist )
				nearbyBadAmount += ( badDist - dist ) / badDist;
			distfromideal = abs( dist - idealDist );
			totalDistFromIdeal += distfromideal;
		}
		avgDistFromIdeal = totalDistFromIdeal / alive.size;
		wellDistancedAmount = ( idealDist - avgDistFromIdeal ) / idealDist;
		weights[i] = wellDistancedAmount - nearbyBadAmount * 2;
	}
	best = 0;
	for ( i = 1; i < spawnpoints.size; i++ )
		if ( weights[i] > weights[best] )
			best = i;
	self.lastspawnpoint = spawnpoints[best];
	return best;
}
"#;

const PLAYERS: u32 = 60;
const SPAWNS: u32 = 40;
/// Entities that are neither, which `getentarray` walks past.
const OTHERS: u32 = 400;

struct Source;

impl ScriptSource for Source {
    fn read(&self, canonical: &str) -> Option<String> {
        (canonical == PATH).then(|| SCRIPT.to_string())
    }
}

/// Entity `n`: a player under `PLAYERS`, then the spawn points, then the
/// rest; each has a classname, an origin and the script fields it was
/// given.
struct BenchHost {
    classname: Vec<&'static str>,
    origin: Vec<[f32; 3]>,
    fields: std::collections::HashMap<(u32, Atom), Value>,
}

impl BenchHost {
    fn new() -> BenchHost {
        let n = PLAYERS + SPAWNS + OTHERS;
        let mut h = BenchHost {
            classname: Vec::new(),
            origin: Vec::new(),
            fields: Default::default(),
        };
        for i in 0..n {
            h.classname.push(if i < PLAYERS {
                "player"
            } else if i < PLAYERS + SPAWNS {
                "mp_dm_spawn"
            } else {
                "script_origin"
            });
            let f = i as f32;
            h.origin.push([
                (f * 397.0) % 3000.0,
                (f * 211.0) % 2500.0,
                (f * 13.0) % 200.0,
            ]);
        }
        h
    }
}

impl Host for BenchHost {
    fn builtin(
        &mut self,
        cx: &mut Cx,
        name: Atom,
        _recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind> {
        match cx.resolve_folded(name) {
            "distance" => match args {
                [Value::Vector(a), Value::Vector(b)] => {
                    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                    Ok(Value::Float(
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(),
                    ))
                }
                _ => Err(ErrorKind::BadType("distance takes two vectors")),
            },
            "isdefined" => Ok(Value::Int(!matches!(args, [Value::Undefined]) as i32)),
            "getentarray" => {
                let [Value::String(want), Value::String(_)] = args else {
                    return Err(ErrorKind::BadType("getentarray"));
                };
                let want = cx.resolve(*want).to_string();
                let arr = cx.new_array();
                let mut n = 0;
                for (i, c) in self.classname.iter().enumerate() {
                    let c = cx.intern_exact(c);
                    if cx.resolve(c) == want {
                        cx.set_index(
                            arr,
                            vcod_gsc::ArrayKey::Int(n),
                            Value::Entity(EntId(i as u32, 0)),
                        );
                        n += 1;
                    }
                }
                Ok(Value::Array(arr))
            }
            _ => Err(ErrorKind::MissingBuiltin(name)),
        }
    }

    fn get_field(&mut self, cx: &mut Cx, e: EntId, f: Atom) -> Value {
        match cx.resolve_folded(f) {
            "origin" => Value::Vector(self.origin[e.0 as usize]),
            "sessionstate" => Value::String(cx.intern_exact("playing")),
            _ => self
                .fields
                .get(&(e.0, f))
                .copied()
                .unwrap_or(Value::Undefined),
        }
    }

    fn set_field(&mut self, _cx: &mut Cx, e: EntId, f: Atom, v: Value) -> Result<(), ErrorKind> {
        self.fields.insert((e.0, f), v);
        Ok(())
    }
}

#[test]
#[ignore = "a benchmark; run in release with --nocapture"]
fn spawn_wave() {
    let mut vm = Vm::new();
    Loader::with_dialect(Box::new(Source), Dialect::Cod2)
        .load(&mut vm, PATH)
        .unwrap_or_else(|e| panic!("compile: {} (line {})", e.msg, e.line));
    vm.apply_profile(&VmProfile::COD2);
    vm.set_budget(u32::MAX);
    let main = vm.func_ref(PATH, "main");
    let mut host = BenchHost::new();
    let mut best = f64::MAX;
    let mut result = None;
    for _ in 0..std::env::var("BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
    {
        let t0 = std::time::Instant::now();
        let v = vm
            .call_now(&mut host, main, None, vec![])
            .expect("the wave runs");
        best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
        assert!(result.is_none_or(|r| r == v));
        result = Some(v);
    }
    println!(
        "bench spawn_wave: {PLAYERS} spawns x {SPAWNS} points x {PLAYERS} players: best of 10 {best:.2} ms, result {result:?}"
    );
}
