//	The lookat trigger's fires and isLookingAt's answer, one logPrint each,
//	for the A/B in crates/server/tests/sd_plant_ab.rs. Run by
//	tools/run_probe.sh with the --probe-plant / --probe-defuse pair on the
//	server; the client halves write their own fixtures.

main()
{
	thread watch_lookats();
	thread watch_teleports();
	maps\mp\gametypes\sd::main();
}

watch_lookats()
{
	wait 1;
	ents = getentarray("trigger_lookat", "classname");
	for (i = 0; i < ents.size; i++)
	{
		num = ents[i] getEntityNumber();
		logPrint("PROBE watch " + num + " trigger_lookat\n");
		ents[i] thread watch_one(num);
		ents[i] thread poll_lookingat(num);
	}
	logPrint("PROBE lookats " + ents.size + "\n");
}

//	One line per "trigger" notify: the trigger, the toucher, where it stood,
//	and the server clock, which is what pairs a fire with the probe's own
//	!trace line.
watch_one(num)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", other);
		logPrint("PROBE fire " + num + " " + getTime() + " " + other getEntityNumber() + " " + other.origin + "\n");
	}
}

//	Once a server frame, every player isLookingAt answers true for.
poll_lookingat(num)
{
	self endon("death");
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (players[i] islookingat(self))
				logPrint("PROBE looking " + num + " " + getTime() + " " + players[i] getEntityNumber() + "\n");
		}
		wait 0.05;
	}
}

//	The two probe clients spawn a town away from bombzone_A on mp_carentan --
//	the allied S&D spawns sit ~3500 units off, and a steered walk spends its
//	whole run in the streets -- so with `probe_teleport 1` each player is put
//	once per level on a teamdeathmatch spawn in the zone's own courtyard. The
//	A/B gate places its clients itself and never sets the cvar, so this thread
//	returns at the first line there; an unset cvar reads "", as sd.gsc relies on.
watch_teleports()
{
	if (getcvar("probe_teleport") != "1")
		return;
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	//	`level` is new after a restart, so the flags clear with it and a
	//	restarted round teleports each player again once it has respawned.
	level.probe_tp = [];
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			player = players[i];
			player try_teleport(player getEntityNumber());
		}
		wait 1;
	}
}

//	Early returns rather than a `continue`: one per reason this player is not
//	ready to be moved yet.
try_teleport(num)
{
	if (isdefined(level.probe_tp[num]))
		return;
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (!isalive(self))
		return;
	if (!isdefined(self.pers["team"]))
		return;

	//	Both are mp_carentan mp_teamdeathmatch_spawn origins, 416 and 541
	//	units from bombzone_A.
	if (self.pers["team"] == game["attackers"])
		dest = (-512, 2688, -16);
	else
		dest = (216, 2088, -8);

	self setOrigin(dest);
	level.probe_tp[num] = 1;
	logPrint("PROBE teleport " + num + " " + self.pers["team"] + " " + dest + "\n");
}
