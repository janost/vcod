//	`func_door`, `func_door_rotating` and `func_rotating` on mp_carentan
//	under stock dm, the stock blocks of *4, *5, *6, *7, *8 and *10 replaced by
//	crates/server/tests/fixtures/doors/mp_carentan-doors.ent (the fixture
//	header has the recipe; docs/research/cod11-movers.md 26):
//
//	door_a  *5   a func_door with a touch trigger (spawnflags 8)
//	door_b  *6   a func_door, master of team d2, dmg 5
//	door_b4 *4   a func_door, slave of d2
//	door_r  *8   a func_door_rotating on the defaults
//	rot1    *7   a func_rotating, master of team r1
//	rot2    *10  a func_rotating, slave of r1
//
//	One `--probe-uo-weapon direct` axis client; the script holds his keys
//	through `iPrintLn("PROBE_IN <buttons> 0 0")` and takes no damage.
//	He stands in door_a's trigger, under none of its brushes, 5 s, stands on door_b and presses use,
//	stands under door_b while it closes, then useBys door_r from two
//	spots. Every frame from 1000 logs each mover and the player:
//
//	PROBE f <time> <a> <b> <b4> <r angles> <rot1 angles> <rot2 angles> <player>
//
//	The player's origin is "none" until his first placing: dm's spawn
//	point is a random pick.
//	PROBE spot <name> <origin>
//	PROBE phase <name> <time>
//	PROBE trigger <name> <time>
//	PROBE dmg <time> <damage> <mod> <attacker classname>
//	PROBE done

main()
{
	thread run();
	maps\mp\gametypes\dm::main();
	level.callbackPlayerDamage = ::on_damage;
}

on_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	c = "none";
	if (isdefined(eAttacker))
		c = eAttacker.classname;
	logPrint("PROBE dmg " + gettime() + " " + iDamage + " " + sMeansOfDeath + " " + c + "\n");
	level.probe_dmg++;
}

in(buttons)
{
	level.probe_player iprintln("PROBE_IN " + buttons + " 0 0");
}

ground_at(p, name)
{
	tr = bullettrace(p, p + (0, 0, -512), false, undefined);
	at = tr["position"];
	logPrint("PROBE spot " + name + " " + at + "\n");
	return at;
}

ent(name)
{
	return getent(name, "targetname");
}

watch_trigger(name)
{
	e = ent(name);
	for (;;)
	{
		e waittill("trigger", who);
		logPrint("PROBE trigger " + name + " " + gettime() + "\n");
	}
}

log_frames()
{
	a = ent("door_a");
	b = ent("door_b");
	b4 = ent("door_b4");
	r = ent("door_r");
	r1 = ent("rot1");
	r2 = ent("rot2");
	for (;;)
	{
		p = "none";
		if (isdefined(level.probe_placed))
			p = level.probe_player.origin;
		logPrint("PROBE f " + gettime() + " " + a.origin + " " + b.origin + " " + b4.origin + " " + r.angles + " " + r1.angles + " " + r2.angles + " " + p + "\n");
		wait 0.05;
	}
}

place(p, angles)
{
	level.probe_placed = true;
	level.probe_player setorigin(p);
	level.probe_player setplayerangles(angles);
}

run()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	level.probe_dmg = 0;
	wait 1;
	thread log_frames();
	thread watch_trigger("door_a");
	thread watch_trigger("door_b");
	thread watch_trigger("door_r");
	spot_a = ground_at((-60, 2320, 0), "a");
	spot_side = ground_at((1950, 2046, 150), "side");
	spot_under = ground_at((1782, 2046, -10), "under");
	spot_d = ground_at((595, 2137, 400), "d");
	spot_e = ground_at((195, 2137, 400), "e");
	for (;;)
	{
		pl = getentarray("player", "classname");
		if (pl.size > 0 && pl[0].sessionstate == "playing")
			break;
		wait 0.05;
	}
	level.probe_player = pl[0];
	in(0);
	wait 0.5;

	logPrint("PROBE phase touch " + gettime() + "\n");
	place(spot_a, (0, 0, 0));
	wait 5;

	logPrint("PROBE phase use " + gettime() + "\n");
	place((1782, 2046, 72), (85, 0, 0));
	wait 0.5;
	in(64);
	wait 0.1;
	in(0);
	wait 4;

	logPrint("PROBE phase block " + gettime() + "\n");
	place(spot_side, (0, 180, 0));
	wait 0.1;
	ent("door_b") useBy(level.probe_player);
	wait 0.7;
	place(spot_under, (0, 180, 0));
	for (i = 0; i < 160 && level.probe_dmg < 2; i++)
		wait 0.05;
	place(spot_side, (0, 180, 0));
	wait 3;

	logPrint("PROBE phase rotate " + gettime() + "\n");
	place(spot_d, (0, 180, 0));
	wait 0.1;
	ent("door_r") useBy(level.probe_player);
	wait 4.5;
	place(spot_e, (0, 0, 0));
	wait 0.1;
	ent("door_r") useBy(level.probe_player);
	wait 4.5;
	logPrint("PROBE done\n");
}
