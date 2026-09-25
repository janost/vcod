//	Mounted MG's server half: places a gunner behind mp_carentan's turret at
//	(1712 1830 8) and a target in front of it, logs every turret once. The
//	hits themselves are the engine's D;/K; lines. Run by tools/run_probe.sh
//	with a --save-turret client (allies) and a --probe-team axis client.

main()
{
	thread log_turrets();
	thread watch_teleports();
	thread watch_delete();
	maps\mp\gametypes\dm::main();
}

//	With `probe_delete_after N`: the capture's gun is deleted N seconds in.
//	Only the A/B rig sets it (the release-on-delete test); retail runs leave
//	it unset.
watch_delete()
{
	secs = getcvarint("probe_delete_after");
	if (secs <= 0)
		return;
	wait secs;
	gun = probe_gun();
	if (isdefined(gun))
		gun delete();
}

log_turrets()
{
	wait 1;
	guns = getentarray("misc_mg42", "classname");
	for (i = 0; i < guns.size; i++)
		logPrint("PROBE turret " + guns[i] getEntityNumber() + " " + guns[i].origin + " " + guns[i].angles + "\n");
}

//	The gun the capture uses: the one nearest (1712 1830 8).
probe_gun()
{
	guns = getentarray("misc_mg42", "classname");
	best = undefined;
	for (i = 0; i < guns.size; i++)
	{
		if (!isdefined(best) || distance(guns[i].origin, (1712, 1830, 8)) < distance(best.origin, (1712, 1830, 8)))
			best = guns[i];
	}
	return best;
}

//	With `probe_teleport 1`: every allied player is put 40 units behind the
//	gun facing along it, every axis player 300 units in front facing back,
//	once per spawn.
watch_teleports()
{
	if (getcvar("probe_teleport") != "1")
		return;
	wait 1;
	gun = probe_gun();
	if (!isdefined(gun))
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place(gun);
		wait 0.05;
	}
}

//	Early returns rather than one compound test, the way probe_pickup's
//	try_teleport guards sessionstate: it is undefined before the first spawn.
try_place(gun)
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
	fwd = anglestoforward(gun.angles);
	if (self.pers["team"] == "allies")
	{
		spot = gun.origin - (fwd[0] * 40, fwd[1] * 40, 0);
		yaw = gun.angles[1];
	}
	else
	{
		spot = gun.origin + (fwd[0] * 300, fwd[1] * 300, 0);
		yaw = gun.angles[1] + 180;
	}
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
	logPrint("PROBE place " + self getEntityNumber() + " " + self.pers["team"] + " " + spot + " " + yaw + "\n");
}
