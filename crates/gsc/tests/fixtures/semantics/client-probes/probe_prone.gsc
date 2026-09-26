//	Prone slope capture's server half. Under probe_teleport 1 it puts every
//	allied player on a mp_carentan grade facing uphill, once per spawn:
//	probe_spot street is the ~4 degree street at (900 1930), mound the ~19
//	degree terrain mound at (-224 60). Run by tools/run_probe.sh with a
//	--save-slope --probe-prone client.

main()
{
	thread watch_teleports();
	maps\mp\gametypes\dm::main();
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
	if (self.pers["team"] != "allies")
		return;
	self.probe_placed = 1;
	//	A few units above the ground, so the placement never starts solid.
	if (getcvar("probe_spot") == "mound")
	{
		spot = (-224, 60, 45);
		yaw = 270;
	}
	else
	{
		spot = (900, 1930, -38);
		yaw = 90;
	}
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
	logPrint("PROBE place " + getTime() + " " + self getEntityNumber() + " " + spot + " " + yaw + "\n");
}
