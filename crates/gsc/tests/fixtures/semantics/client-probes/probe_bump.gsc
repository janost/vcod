//	Player clip's server half. Under probe_teleport 1 it puts every axis
//	player on a flat brush floor on mp_carentan and every allied player 200
//	units behind it along -x, both facing +x, once per spawn. Under
//	probe_overlap 1 it then setorigins the axis player onto the allied one 12 s
//	and 30 s after both are placed, and logs both players' origins every frame
//	from 1 s before to 3 s after each. Run by tools/run_probe.sh with a
//	--probe-bump-target client (axis) and a --save-bump client (allies).

main()
{
	thread watch_teleports();
	thread watch_overlap();
	maps\mp\gametypes\dm::main();
}

//	The target's feet. The floor is flat brush from 280 units behind it to 360
//	in front and 70 either side.
bump_spot()
{
	return (1132, -376, -151.875);
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
	spot = bump_spot();
	if (self.pers["team"] == "allies")
		spot = spot - (200, 0, 0);
	else if (self.pers["team"] != "axis")
		return;
	self.probe_placed = 1;
	self setorigin(spot);
	self setplayerangles((0, 0, 0));
	logPrint("PROBE place " + getTime() + " " + self getEntityNumber() + " " + self.pers["team"] + " " + spot + " 0\n");
}

//	The placed, playing player on `team`, or undefined.
placed_player(team)
{
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		if (!isdefined(p.probe_placed))
			continue;
		if (!isdefined(p.sessionstate))
			continue;
		if (p.sessionstate != "playing")
			continue;
		if (!isdefined(p.pers["team"]))
			continue;
		if (p.pers["team"] == team)
			return p;
	}
	return undefined;
}

watch_overlap()
{
	if (getcvar("probe_overlap") != "1")
		return;
	for (;;)
	{
		walker = placed_player("allies");
		target = placed_player("axis");
		if (isdefined(walker) && isdefined(target))
			break;
		wait 0.05;
	}
	t0 = getTime();
	logPrint("PROBE both_placed " + t0 + "\n");
	overlap_at(t0, 12000);
	overlap_at(t0, 30000);
	logPrint("PROBE overlap_done " + getTime() + "\n");
}

//	One overlap `at` ms after `t0`, with the per-frame log around it.
overlap_at(t0, at)
{
	while (getTime() < t0 + at - 1000)
		wait 0.05;
	logPrint("PROBE at log_states\n");
	while (getTime() < t0 + at)
	{
		log_states();
		wait 0.05;
	}
	walker = placed_player("allies");
	target = placed_player("axis");
	if (isdefined(walker) && isdefined(target))
	{
		logPrint("PROBE at overlap_setorigin\n");
		spot = walker.origin;
		target setorigin(spot);
		logPrint("PROBE overlap " + getTime() + " " + target getEntityNumber() + " " + walker getEntityNumber() + " " + spot + "\n");
	}
	else
		logPrint("PROBE overlap_missed " + getTime() + "\n");
	while (getTime() < t0 + at + 3000)
	{
		log_states();
		wait 0.05;
	}
}

log_states()
{
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		if (!isdefined(p.sessionstate))
			continue;
		if (p.sessionstate != "playing")
			continue;
		logPrint("PROBE state " + getTime() + " " + p getEntityNumber() + " " + p.pers["team"] + " " + p.origin + " " + p isonground() + "\n");
	}
}
