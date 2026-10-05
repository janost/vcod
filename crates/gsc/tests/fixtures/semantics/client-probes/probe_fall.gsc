//	Landing stun capture's server half. Under probe_teleport 1 it drops every
//	allied player onto the mp_carentan street at (900 1930) from each height
//	in turn, 8 s apart, with health reset to 100 before each drop, and logs
//	the health 4 s after. Run by tools/run_probe.sh with a --probe-fall
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
	heights[4] = 520;
	wait 1;
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
		self.health = 100;
		spot = (900, 1930, -38 + heights[i]);
		self setorigin(spot);
		self setplayerangles((0, 90, 0));
		logPrint("PROBE drop " + getTime() + " " + heights[i] + " " + spot + "\n");
		wait 4;
		logPrint("PROBE after " + getTime() + " " + heights[i] + " health " + self.health + " " + self.origin + "\n");
		wait 4;
	}
	logPrint("PROBE done " + getTime() + "\n");
}
