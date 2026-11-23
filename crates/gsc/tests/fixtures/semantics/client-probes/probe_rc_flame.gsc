//	A retail United Offensive client's `fdc` cadence while it stands in a
//	flamethrower's stream (docs/research/coduo-diff.md 19.3, "The flame
//	report"), the server half of tools/capture/retail-client step `fdc`.
//	probe_flame's pair with the roles swapped: the first client to join
//	is the victim (the retail client), the second the shooter (vcod's
//	--probe-uo-weapon direct client), and nobody takes damage, so the
//	victim keeps reporting for the whole run. UO dm only.
//
//	Every client that reaches the spectator state is put on allies with
//	the m1 carbine and spawned through dm's own spawnPlayer, so the retail
//	client clicks no menu:
//
//	PROBE joined <time> <victim|shooter> <entity number>
//
//	The shooter gets the flamethrower and a switch to it. Once both play,
//	the shooter is set on the first deathmatch spawn with a clear line
//	along +x, facing it, and the victim 100 units along it, both put back
//	every 2 s; from then the shooter holds the trigger 0.2 s of every
//	0.5 s through `PROBE_IN` lines:
//
//	PROBE place <time> <shooter spot>
//	PROBE pulse <time> <n>
//
//	The damage callback logs what the engine hands it and applies none:
//
//	PROBE damage <time> victim <ent> attacker <ent> damage <n> mod <mod> weapon <weapon>

main()
{
	precacheItem("flamethrower_mp");
	thread autojoin("m1carbine_mp");
	thread watch();
	maps\mp\gametypes\dm::main();
}

//	dm's connect leaves a client spectating with its team menu open; this
//	answers for it.
autojoin(weapon)
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (!isdefined(p.sessionstate) || p.sessionstate != "spectator")
				continue;
			if (isdefined(p.rc_joined))
				continue;
			p.rc_joined = true;
			p.pers["team"] = "allies";
			p.pers["weapon"] = weapon;
			p closeMenu();
			p maps\mp\gametypes\dm::spawnPlayer();
			if (!isdefined(level.rc_victim))
			{
				level.rc_victim = p;
				role = "victim";
			}
			else if (!isdefined(level.rc_shooter))
			{
				level.rc_shooter = p;
				role = "shooter";
				p thread arm();
			}
			else
				role = "other";
			logPrint("PROBE joined " + getTime() + " " + role + " " + p getEntityNumber() + "\n");
		}
		wait 0.05;
	}
}

arm()
{
	self giveWeapon("flamethrower_mp");
	self giveMaxAmmo("flamethrower_mp");
	// A select sent with the spawn reaches the client before its first
	// playing snapshot, and is dropped.
	wait 1;
	self switchToWeapon("flamethrower_mp");
}

watch()
{
	wait 1;
	level.callbackPlayerDamage = ::probe_damage;
	for (;;)
	{
		v = level.rc_victim;
		s = level.rc_shooter;
		if (isdefined(v) && isdefined(s) && v.sessionstate == "playing" && s.sessionstate == "playing")
			break;
		wait 0.05;
	}
	level.rc_shooter thread pulses();
	for (;;)
	{
		if (!isdefined(level.rc_victim) || !isdefined(level.rc_shooter))
			return;
		place(level.rc_shooter, level.rc_victim);
		wait 2;
	}
}

pulses()
{
	self endon("disconnect");
	wait 3;
	for (n = 0; ; n++)
	{
		self iprintln("PROBE_IN 1 0 0");
		logPrint("PROBE pulse " + getTime() + " " + n + "\n");
		wait 0.2;
		self iprintln("PROBE_IN 0 0 0");
		wait 0.3;
	}
}

place(shooter, victim)
{
	if (!isdefined(level.rc_spot))
	{
		spots = getentarray("mp_deathmatch_spawn", "classname");
		for (k = 0; k < spots.size; k++)
		{
			at = spots[k].origin;
			eye = at + (0, 0, 60);
			ahead = bulletTrace(eye, eye + (160, 0, 0), false, undefined);
			floor = bulletTrace(at + (100, 0, 48), at + (100, 0, -64), false, undefined);
			if (ahead["fraction"] == 1 && floor["fraction"] < 1)
			{
				level.rc_spot = at;
				break;
			}
		}
		if (!isdefined(level.rc_spot))
		{
			logPrint("PROBE no clear spot\n");
			level.rc_spot = shooter.origin;
		}
	}
	shooter setOrigin(level.rc_spot);
	shooter setPlayerAngles((0, 0, 0));
	victim setOrigin(level.rc_spot + (100, 0, 0));
	logPrint("PROBE place " + getTime() + " " + level.rc_spot + "\n");
}

show_ent(e)
{
	if (!isdefined(e))
		return "undefined";
	return "ent" + e getEntityNumber();
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE damage " + getTime() + " victim " + show_ent(self) + " attacker " + show_ent(eAttacker) + " damage " + iDamage + " mod " + sMeansOfDeath + " weapon " + sWeapon + "\n");
}
