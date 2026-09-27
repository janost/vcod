//	A round through a window pane. Under probe_teleport 1 on mp_depot it puts
//	every axis player 76 units behind the glass brush at x -772..-764 (whose
//	outer face draws a SOLID-contents material) and every allied player 16
//	units in front of it, 100 apart along -x, once per spawn. Every damage
//	callback is logged ahead of dm's own. Run by tools/run_probe.sh with one
//	--probe-target client (axis) started first and one --save-hit
//	--probe-sweep client (allies).

main()
{
	thread watch_teleports();
	maps\mp\gametypes\dm::main();
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::probe_damage;
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE damage " + getTime() + " " + self getEntityNumber() + " " + iDamage + " " + iDFlags + " " + sMeansOfDeath + " " + sHitLoc + " " + vPoint + " " + self.origin + "\n");
	self [[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
}

watch_teleports()
{
	if (getcvar("probe_teleport") != "1")
		return;
	if (getcvar("mapname") != "mp_depot")
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place();
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_place()
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
	if (self.pers["team"] == "allies")
	{
		spot = (-748, -3656, -39.875);
		yaw = 180;
	}
	else if (self.pers["team"] == "axis")
	{
		spot = (-848, -3656, -39.875);
		yaw = 0;
	}
	else
		return;
	self.probe_placed = true;
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
	logPrint("PROBE place " + getTime() + " " + self getEntityNumber() + " " + self.pers["team"] + " " + spot + "\n");
}
