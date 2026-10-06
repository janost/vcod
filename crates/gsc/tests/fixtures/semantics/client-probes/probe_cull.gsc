//	Snapshot cull of a moving brush model, mp_carentan only. Keeps the
//	bombzone_A brush model *5 (dm's _gameobjects would delete it), stands
//	every allied player on the attackers' spawn beside it, and lifts it 20000
//	units over 8 s, out of the world, then lowers it back over 8 s. Every
//	server frame of a phase logs the mover's getorigin():
//
//		PROBE f <phase> <time> <mover origin>
//
//	Run by tools/run_probe.sh with a --probe-ride --probe-team allies client,
//	whose RIDE_GONE and RIDE_ENT lines say on which snapshot the brush model
//	left and came back. A cull at trBase keeps it for the whole lift and
//	drops it for the whole descent; a cull where it is drops it once its box
//	clears the world and brings it back once the descent brings it in.

main()
{
	bms = getentarray("script_brushmodel", "classname");
	for (i = 0; i < bms.size; i++)
		bms[i].script_gameobjectname = "dm";
	thread watch_players();
	maps\mp\gametypes\dm::main();
}

watch_players()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	bms = getentarray("script_brushmodel", "classname");
	bz = undefined;
	for (i = 0; i < bms.size; i++)
		if (!isdefined(bz) || bms[i] getEntityNumber() < bz getEntityNumber())
			bz = bms[i];
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_cull(bz);
		wait 0.05;
	}
}

try_cull(bz)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_culling))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_culling = 1;
	self thread phases(bz);
}

phases(bz)
{
	self setorigin((-512, 2688, -16));
	self setplayerangles((0, 90, 0));
	wait 3;
	logPrint("PROBE bm " + bz getEntityNumber() + "\n");
	logPrint("PROBE at cull_up " + getTime() + "\n");
	bz movez(20000, 8);
	sample("cull_up", bz, 9);
	logPrint("PROBE at cull_down " + getTime() + "\n");
	bz movez(-20000, 8);
	sample("cull_down", bz, 9);
	logPrint("PROBE done " + getTime() + "\n");
}

sample(phase, ent, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE f " + phase + " " + getTime() + " " + ent getorigin() + "\n");
		wait 0.05;
	}
}
