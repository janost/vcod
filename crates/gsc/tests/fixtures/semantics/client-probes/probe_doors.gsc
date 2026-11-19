//	`func_door`, `func_door_rotating` and `func_rotating` on mp_carentan
//	under stock dm, the stock blocks of *4, *5, *6, *7, *8 and *10 replaced by
//	crates/server/tests/fixtures/doors/mp_carentan-doors.ent (the fixture
//	header has the recipe; docs/research/cod11-movers.md 26):
//
//	door_a  *5   a func_door with a touch trigger (spawnflags 8), light and color
//	door_b  *6   a func_door, master of team d2, dmg 5, health
//	door_b4 *4   a func_door, slave of d2, color
//	door_r  *8   a func_door_rotating on the defaults, health
//	rot1    *7   a func_rotating, master of team r1, light
//	rot2    *10  a func_rotating, slave of r1
//	door_h  *9   a func_door with health and no angles
//
//	One `--probe-uo-weapon direct` axis client; the script holds his keys
//	through `iPrintLn("PROBE_IN <buttons> 0 0")` and takes no damage.
//	He stands in door_a's trigger, under none of its brushes, 5 s, stands on door_b and presses use,
//	stands under door_b while it closes, then useBys door_r from two
//	spots. Then door_r twice more, the player taken far off while it shuts
//	(the PVS test of its sounds), radiusDamage on door_r, door_b, door_h
//	shut and door_h moving, and a script spawn of each classname at the world
//	origin. Then he shoots door_h at rest from 80 units south (a round's
//	G_Damage; door_b's brushes stop no bulletTrace), then swings at it from 50 units
//	south (a melee's G_Damage, which has no glass test). Ahead of all that, door_h takes a radiusDamage at 550 and
//	another at 4000, inside the level's first 4000 ms of silent door uses.
//	Every frame from 1000 logs each mover and the player:
//
//	PROBE f <time> <a> <b> <b4> <h> <r angles> <rot1 angles> <rot2 angles> <player>
//
//	The player's origin is "none" until his first placing: dm's spawn
//	point is a random pick.
//	PROBE spot <name> <origin>
//	PROBE phase <name> <time>
//	PROBE trigger <name> <time>
//	PROBE dmg <time> <damage> <mod> <attacker classname>
//	PROBE spawned <classname> <origin> <angles> <speed> <dmg>
//	PROBE s <time> <door origin> <door_rotating angles> <rotating angles>
//	PROBE early <time>
//	PROBE aim <name> <eye> <target> <what the eye's bulletTrace meets> <where>
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

// door_h opened by blasts before the client is in: the first at 550, the
// second at 4000, the last level time Use_BinaryMover keeps silent.
early()
{
	wait 0.5;
	logPrint("PROBE early " + gettime() + "\n");
	radiusDamage((1053, 1611, 82), 100, 50, 50);
	wait 3.45;
	logPrint("PROBE early " + gettime() + "\n");
	radiusDamage((1053, 1611, 82), 100, 50, 50);
}

// Stands the player at `spot` looking at `target` and taps `buttons` (1
// fires one round, 32 swings).
shoot(spot, target, name, buttons)
{
	eye = spot + (0, 0, 60);
	place(spot, vectortoangles(target - eye));
	tr = bullettrace(eye, target, false, level.probe_player);
	what = "world";
	if (isdefined(tr["entity"]))
	{
		what = tr["entity"].classname;
		if (isdefined(tr["entity"].targetname))
			what = tr["entity"].targetname;
	}
	logPrint("PROBE aim " + name + " " + eye + " " + target + " " + what + " " + tr["position"] + "\n");
	wait 0.5;
	in(buttons);
	wait 0.1;
	in(0);
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
	h = ent("door_h");
	r = ent("door_r");
	r1 = ent("rot1");
	r2 = ent("rot2");
	for (;;)
	{
		p = "none";
		if (isdefined(level.probe_placed))
			p = level.probe_player.origin;
		logPrint("PROBE f " + gettime() + " " + a.origin + " " + b.origin + " " + b4.origin + " " + h.origin + " " + r.angles + " " + r1.angles + " " + r2.angles + " " + p + "\n");
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
	thread early();
	wait 1;
	thread log_frames();
	thread watch_trigger("door_a");
	thread watch_trigger("door_b");
	thread watch_trigger("door_r");
	thread watch_trigger("door_h");
	spot_a = ground_at((-60, 2320, 0), "a");
	spot_side = ground_at((1950, 2046, 150), "side");
	spot_under = ground_at((1782, 2046, -10), "under");
	spot_d = ground_at((595, 2137, 400), "d");
	spot_e = ground_at((195, 2137, 400), "e");
	spot_far = ground_at((296, -1280, 100), "far");
	spot_h = ground_at((1053, 1531, 120), "h");
	spot_k = ground_at((1053, 1561, 120), "k");
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

	logPrint("PROBE phase pvs " + gettime() + "\n");
	place(spot_d, (0, 180, 0));
	wait 0.1;
	ent("door_r") useBy(level.probe_player);
	wait 2.5;
	place(spot_far, (0, 0, 0));
	wait 0.9;
	place(spot_d, (0, 180, 0));
	wait 3;
	ent("door_r") useBy(level.probe_player);
	wait 2.5;
	place(spot_far, (0, 0, 0));
	wait 3;
	place(spot_d, (0, 180, 0));
	wait 1;

	logPrint("PROBE phase damage " + gettime() + "\n");
	place(spot_far, (0, 0, 0));
	wait 0.5;
	radiusDamage((397, 2135, 262), 200, 50, 50);
	wait 0.5;
	radiusDamage((1782, 2046, 30), 200, 50, 50);
	wait 0.5;
	radiusDamage((1053, 1611, 82), 100, 50, 50);
	wait 0.1;
	radiusDamage((1053, 1611, 82), 100, 50, 50);
	wait 3;

	logPrint("PROBE phase spawn " + gettime() + "\n");
	s1 = spawn("func_door", (0, 0, 0));
	s2 = spawn("func_door_rotating", (0, 0, 0));
	s3 = spawn("func_rotating", (0, 0, 0));
	logPrint("PROBE spawned " + s1.classname + " " + s1.origin + " " + s1.angles + " " + s1.speed + " " + s1.dmg + "\n");
	logPrint("PROBE spawned " + s2.classname + " " + s2.origin + " " + s2.angles + " " + s2.speed + " " + s2.dmg + "\n");
	logPrint("PROBE spawned " + s3.classname + " " + s3.origin + " " + s3.angles + " " + s3.speed + " " + s3.dmg + "\n");
	for (i = 0; i < 10; i++)
	{
		logPrint("PROBE s " + gettime() + " " + s1.origin + " " + s2.angles + " " + s3.angles + "\n");
		wait 0.05;
	}

	logPrint("PROBE phase shoot " + gettime() + "\n");
	shoot(spot_h, (1053, 1611, 82), "h", 1);
	wait 3;

	logPrint("PROBE phase knife " + gettime() + "\n");
	shoot(spot_k, (1053, 1611, 82), "k", 32);
	wait 3;
	logPrint("PROBE done\n");
}
