//	earthquake's wire half (combat doc 17.2). Every player alive gets
//	earthquake(0.3, 2.5, origin, 850) at its own origin every 2 s, then
//	earthquake(0.05, 0.0026, origin, 100) a frame later, each logged as
//	`PROBE quake <time> <scale> <duration> <radius> <origin>`. The client
//	half is a plain --net-probe, which prints an `EV_EARTHQUAKE` line with
//	the temp entity's angles2 and time.

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
			players[i] try_quake();
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_quake()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_quake))
		return;
	self.probe_quake = 1;
	self thread quakes();
}

quakes()
{
	self endon("disconnect");
	for (;;)
	{
		quake(0.3, 2.5, self.origin, 850);
		wait 0.05;
		quake(0.05, 0.0026, self.origin, 100);
		wait 2;
	}
}

quake(scale, duration, origin, radius)
{
	earthquake(scale, duration, origin, radius);
	logPrint("PROBE quake " + getTime() + " " + scale + " " + duration + " " + radius + " " + origin + "\n");
}
