//	United Offensive: a tank's shell on mp_foy's belltower `trigger_damage`
//	(spawnflags 115, `script_exploder` 2; docs/research/cod11-combat.md
//	14.8.3). No stock tank has a line of fire to the belfry, so the map runs
//	with `crates/server/tests/fixtures/triggers/mp_foy-belltower-tank.ent`
//	appended: one more Panzer IV 900 units north of the tower, facing it.
//	Client 0 is put 110 units off its right flank to board it with
//	`--probe-vehicle sit`; seated, his view is held on the trigger's origin
//	and the gun fires once with `fireTurret` 3 s into the seat. Logged:
//
//	PROBE vehicle <number> <vehicletype> <origin> <angles>
//	PROBE place <client> <spot> <yaw>
//	PROBE seat <vehpos> / PROBE fire <vehicle origin> <vehicle angles>
//	PROBE damage <dmg> <attacker> <mod>  (the trigger's notifies)
//	PROBE trigger <activator>
//	PROBE hurt <dmg> <mod> <attacker>    (a player, the callback only logs)
//	PROBE state <trigger> <tower brush> <fx models>  (each `1` while it exists)
//	PROBE done
//
//	crates/server/tests/dmgtrig_ab.rs holds retail's lines.

main()
{
	thread run();
	maps\mp\gametypes\tdm::main();
	level.callbackPlayerDamage = ::hurt;
}

hurt(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE hurt " + iDamage + " " + sMeansOfDeath + " " + who(eAttacker) + "\n");
}

who(e)
{
	if (!isdefined(e))
		return "undefined";
	if (isplayer(e))
		return "player";
	if (e getEntityNumber() == 1022)
		return "world";
	return e.classname;
}

watch_damage()
{
	self endon("death");
	for (;;)
	{
		self waittill("damage", dmg, attacker, dir, point, mod);
		m = "-";
		if (isdefined(mod))
			m = mod;
		logPrint("PROBE damage " + dmg + " " + who(attacker) + " " + m + "\n");
	}
}

watch_trigger()
{
	self waittill("trigger", other);
	logPrint("PROBE trigger " + who(other) + "\n");
}

state()
{
	tr = 0;
	if (isdefined(getent("belltower_damage", "targetname")))
		tr = 1;
	br = 0;
	if (isdefined(getent("delete", "targetname")))
		br = 1;
	fx = 0;
	m = getentarray("script_model", "classname");
	for (i = 0; i < m.size; i++)
		if (isdefined(m[i].script_exploder) && m[i].script_exploder == 2)
			fx++;
	logPrint("PROBE state " + tr + " " + br + " " + fx + "\n");
}

//	The script_vehicle nearest `at`.
nearest_vehicle(at)
{
	vehs = getentarray("script_vehicle", "classname");
	best = undefined;
	for (i = 0; i < vehs.size; i++)
		if (!isdefined(best) || distance(vehs[i].origin, at) < distance(best.origin, at))
			best = vehs[i];
	return best;
}

watch_seat()
{
	for (;;)
	{
		self waittill("activated", vehpos, activator);
		self.probe_driver = undefined;
		if (vehpos == 1)
			self.probe_driver = activator;
		self.probe_seated_at = gettime();
		logPrint("PROBE seat " + vehpos + "\n");
	}
}

//	Early returns: sessionstate is undefined before the first spawn.
try_place(veh)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
	{
		self.probe_placed = undefined;
		return;
	}
	if (isdefined(self.probe_placed))
		return;
	self.probe_placed = 1;
	right = anglestoright(veh.angles);
	spot = veh.origin + (right[0] * 110, right[1] * 110, 8);
	yaw = vectortoangles(veh.origin - spot)[1];
	self setorigin(spot);
	self setplayerangles((10, yaw, 0));
	logPrint("PROBE place " + self getEntityNumber() + " " + spot + " " + yaw + "\n");
}

run()
{
	wait 2;
	trig = getent("belltower_damage", "targetname");
	trig thread watch_damage();
	trig thread watch_trigger();
	at = trig.origin;
	veh = nearest_vehicle(at);
	logPrint("PROBE vehicle " + veh getEntityNumber() + " " + veh.vehicletype + " " + veh.origin + " " + veh.angles + "\n");
	state();
	veh thread watch_seat();
	fired = undefined;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place(veh);
		if (isdefined(veh.probe_driver))
		{
			driver = veh.probe_driver;
			driver setplayerangles(vectortoangles(at - (driver.origin + (0, 0, 60))));
			if (!isdefined(fired) && gettime() - veh.probe_seated_at >= 3000)
			{
				fired = gettime();
				logPrint("PROBE fire " + veh.origin + " " + veh.angles + "\n");
				veh fireTurret();
			}
		}
		if (isdefined(fired) && gettime() - fired >= 3000)
			break;
		wait 0.05;
	}
	state();
	logPrint("PROBE done\n");
}
