//	Airborne prone refusal capture's server half. Under probe_teleport 1 it
//	lifts every allied player 100 units over the mp_carentan street, facing
//	north, alternately at (900 1680), where the wall to the south leaves no
//	room behind the body, and at (900 1930), in the open, 6 s apart. Run by
//	tools/run_probe.sh with a --probe-fall --probe-fall-prone 90
//	--probe-team allies client, which lies down before the first lift.

main()
{
	thread watch_lifts();
	maps\mp\gametypes\dm::main();
}

watch_lifts()
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
			players[i] try_lifts();
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_lifts()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_lifting))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_lifting = 1;
	self thread lifts();
}

lifts()
{
	wait 4;
	for (i = 0; i < 4; i++)
	{
		if (i % 2 == 0)
			spot = (900, 1680, 62);
		else
			spot = (900, 1930, 62);
		self setorigin(spot);
		self setplayerangles((0, 90, 0));
		logPrint("PROBE lift " + getTime() + " " + spot + "\n");
		wait 6;
	}
	logPrint("PROBE done " + getTime() + "\n");
}
