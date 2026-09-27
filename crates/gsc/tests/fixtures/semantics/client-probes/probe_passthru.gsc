//	Rifle-round pass-through. Under probe_teleport 1 on mp_carentan it puts the
//	lower-numbered axis player on probe_bump's flat brush floor, the other 100
//	units behind it along +x, and every allied player 200 units in front along -x,
//	all facing +x, once per spawn. Every damage callback is logged with the
//	frame's time, the victim, the damage and where it landed, ahead of dm's
//	own. Run by tools/run_probe.sh with two --probe-target clients (axis)
//	started first and one --save-hit --probe-sweep client (allies).

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
	if (getcvar("mapname") != "mp_carentan")
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

//	The axis slot `self` takes: 1 when an axis player with a lower entity
//	number is connected, so the front and the back never swap on a respawn.
axis_slot()
{
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		if (!isdefined(p.pers["team"]))
			continue;
		if (p.pers["team"] != "axis")
			continue;
		if (p getEntityNumber() < self getEntityNumber())
			return 1;
	}
	return 0;
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_place()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
	{
		self.probe_slot = undefined;
		return;
	}
	if (isdefined(self.probe_slot))
		return;
	spot = (1132, -376, -151.875);
	if (self.pers["team"] == "allies")
	{
		self.probe_slot = -1;
		spot = spot - (200, 0, 0);
	}
	else if (self.pers["team"] == "axis")
	{
		self.probe_slot = axis_slot();
		spot = spot + (100 * self.probe_slot, 0, 0);
	}
	else
		return;
	self setorigin(spot);
	self setplayerangles((0, 0, 0));
	logPrint("PROBE place " + getTime() + " " + self getEntityNumber() + " " + self.pers["team"] + " " + spot + "\n");
}
