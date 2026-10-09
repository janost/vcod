//	freezeControls capture's server half. Every player that spawns is frozen
//	with freezeControls(true) on its first frame alive and logged as `PROBE
//	freeze <time> <name>`. Run by tools/run_probe.sh with a --save-motion
//	--capture-tag client, which holds lean, crouch, prone and the runs in turn.

main()
{
	thread watch_spawns();
	maps\mp\gametypes\dm::main();
}

watch_spawns()
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_freeze();
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_freeze()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_frozen))
		return;
	self.probe_frozen = 1;
	self freezecontrols(true);
	logPrint("PROBE freeze " + getTime() + " " + self.name + "\n");
}
