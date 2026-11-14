//	Moving, leaning and deployed shots' server half (docs/research/
//	cod11-combat.md 2.1, "1.5 and United Offensive"). Stock dm on
//	mp_carentan. Under probe_teleport 1 it puts every allied player on a
//	spot facing world yaw 0, once per spawn: probe_spot street is
//	probe_prone's street at (900 1930), the wall 1090 units ahead (its face
//	at x 1991.875); wall is the ground (z -23.875) probe_walldist units
//	(default 20) short of that wall on the same line, close enough that a
//	deployed LMG's muzzle trace meets it. Under +set probe_block <d> the
//	spot gets a script_model barrel d units ahead on the ground (at z
//	probe_block_z instead when set; no model under probe_block_bare 1),
//	its contents set to probe_block_contents (default 1), which the
//	bipod's muzzle trace (mask 0x11) can meet; PROBE block logs it.
//	Run by tools/run_probe.sh with a --save-slope --probe-moveshot client:
//
//	PROBE place <time> <entnum> <spot> <yaw>

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

wall_spot()
{
	d = 20;
	if (getcvar("probe_walldist") != "")
		d = getcvarfloat("probe_walldist");
	x = 1991 - d;
	tr = bullettrace((x, 1930, 40), (x, 1930, -200), false, undefined);
	return tr["position"] + (0, 0, 0.125);
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
	if (getcvar("probe_spot") == "wall")
		spot = wall_spot();
	else
		spot = (900, 1930, -38);
	self setorigin(spot);
	self setplayerangles((0, 0, 0));
	if (getcvar("probe_block") != "" && !isdefined(level.probe_block))
	{
		x = spot[0] + getcvarfloat("probe_block");
		if (getcvar("probe_block_z") != "")
			at = (x, 1930, getcvarfloat("probe_block_z"));
		else
		{
			tr = bullettrace((x, 1930, 40), (x, 1930, -200), false, undefined);
			at = tr["position"];
		}
		level.probe_block = spawn("script_model", at);
		if (getcvar("probe_block_bare") != "1")
			level.probe_block setmodel("xmodel/barrel_black1");
		contents = 1;
		if (getcvar("probe_block_contents") != "")
			contents = getcvarint("probe_block_contents");
		level.probe_block setcontents(contents);
		logPrint("PROBE block " + getTime() + " " + level.probe_block getEntityNumber() + " " + level.probe_block.origin + " " + contents + "\n");
	}
	logPrint("PROBE place " + getTime() + " " + self getEntityNumber() + " " + spot + " 0\n");
}
