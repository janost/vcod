//	A hit's knockback timer (pm_flags 0x200) while the player slides along a
//	wall, the server half. Under probe_teleport 1 it sets every allied player
//	down on mp_carentan street at (960 1830), just north of the street's south
//	wall, 3 s apart, and 0.3 s on (the --probe-fall-walk 315 client already
//	sliding east along the wall) sets off a radiusDamage beside it whose
//	direction the trial names: from the north (into the wall), from the
//	north-west (along and into it), from the east (against the walk), and two
//	from the north one frame apart (the second lands inside the first's
//	timer). maxDamage and minDamage match, so the damage never depends on the
//	distance, and 1000 health keeps every hit alive. The first trial only
//	takes up the client's pre-teleport velocity. Run by tools/run_probe.sh
//	with a --probe-fall --probe-fall-walk 315 --probe-team allies client.

main()
{
	thread watch();
	maps\mp\gametypes\dm::main();
}

watch()
{
	if (getcvar("probe_teleport") != "1")
		return;
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::probe_damage;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_trials();
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_trials()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_running))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_running = 1;
	self thread trials();
}

trials()
{
	// Blast offsets from the player's origin, and each blast's damage. The
	// +24 in z cancels G_RadiusDamage's own lift of the direction.
	names[0] = "north";
	offsets[0] = (0, 40, 24);
	damages[0] = 200;
	names[1] = "north";
	offsets[1] = (0, 40, 24);
	damages[1] = 200;
	names[2] = "north_light";
	offsets[2] = (0, 40, 24);
	damages[2] = 100;
	names[3] = "northwest";
	offsets[3] = (-30, 30, 24);
	damages[3] = 200;
	names[4] = "east";
	offsets[4] = (40, 0, 24);
	damages[4] = 200;
	names[5] = "north_twice";
	offsets[5] = (0, 40, 24);
	damages[5] = 200;
	wait 3;
	for (i = 0; i < names.size; i++)
	{
		self.maxhealth = 1000;
		self.health = 1000;
		spot = (960, 1830, -39);
		self setorigin(spot);
		self setplayerangles((0, 315, 0));
		logPrint("PROBE drop " + getTime() + " " + names[i] + " " + spot + "\n");
		wait 0.3;
		self blast(names[i], offsets[i], damages[i]);
		if (names[i] == "north_twice")
		{
			wait 0.05;
			self blast(names[i], offsets[i], damages[i]);
		}
		wait 2.5;
		logPrint("PROBE after " + getTime() + " " + names[i] + " health " + self.health + " " + self.origin + "\n");
		wait 0.2;
	}
	logPrint("PROBE done " + getTime() + "\n");
}

blast(name, offset, damage)
{
	at = self.origin + offset;
	logPrint("PROBE blast " + getTime() + " " + name + " " + self.origin + " at " + at + " damage " + damage + "\n");
	radiusDamage(at, 100, damage, damage);
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE damage " + getTime() + " damage " + iDamage + " dflags " + iDFlags + " mod " + sMeansOfDeath + " dir " + vDir + " health " + self.health + "\n");
	[[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
	logPrint("PROBE damaged " + getTime() + " health " + self.health + "\n");
}
