//	Landing stun and fall damage capture's server half. Under probe_teleport 1
//	it drops every allied player onto the mp_carentan street at (900 1930)
//	from each height in turn, 8 s apart, with health reset before each drop,
//	and logs the health 4 s after. The second 340 drop runs at maxhealth 200,
//	which shows what the damage is a percentage of; the fatal 520 stays last.
//	The damage and killed callbacks are wrapped to log the arguments the
//	engine hands them. Run by tools/run_probe.sh with a --probe-fall
//	--probe-team allies client.

main()
{
	thread watch_drops();
	maps\mp\gametypes\dm::main();
}

watch_drops()
{
	if (getcvar("probe_teleport") != "1")
		return;
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	heights[0] = 100;
	heights[1] = 300;
	heights[2] = 340;
	heights[3] = 420;
	heights[4] = 340;
	heights[5] = 520;
	wait 1;
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::probe_damage;
	level.probe_killed = level.callbackPlayerKilled;
	level.callbackPlayerKilled = ::probe_killed;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_drops(heights);
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_drops(heights)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_dropping))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_dropping = 1;
	self thread drops(heights);
}

drops(heights)
{
	wait 3;
	for (i = 0; i < heights.size; i++)
	{
		if (i == 4)
			self.maxhealth = 200;
		else
			self.maxhealth = 100;
		self.health = self.maxhealth;
		spot = (900, 1930, -38 + heights[i]);
		self setorigin(spot);
		self setplayerangles((0, 90, 0));
		logPrint("PROBE drop " + getTime() + " " + heights[i] + " " + spot + " maxhealth " + self.maxhealth + "\n");
		wait 4;
		logPrint("PROBE after " + getTime() + " " + heights[i] + " health " + self.health + " " + self.origin + "\n");
		wait 4;
	}
	logPrint("PROBE done " + getTime() + "\n");
}

show(v)
{
	if (!isdefined(v))
		return "undefined";
	return "" + v;
}

show_ent(e)
{
	if (!isdefined(e))
		return "undefined";
	return "ent" + e getEntityNumber();
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE damage " + getTime() + " inflictor " + show_ent(eInflictor) + " attacker " + show_ent(eAttacker) + " damage " + iDamage + " dflags " + iDFlags + " mod " + sMeansOfDeath + " weapon " + sWeapon + " point " + show(vPoint) + " dir " + show(vDir) + " hitloc " + sHitLoc + " health " + self.health + " maxhealth " + self.maxhealth + "\n");
	[[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
	logPrint("PROBE damaged " + getTime() + " health " + self.health + " " + self.sessionstate + "\n");
}

probe_killed(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc)
{
	logPrint("PROBE killed " + getTime() + " inflictor " + show_ent(eInflictor) + " attacker " + show_ent(eAttacker) + " damage " + iDamage + " mod " + sMeansOfDeath + " weapon " + sWeapon + " dir " + show(vDir) + " hitloc " + sHitLoc + "\n");
	[[level.probe_killed]](eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc);
}
