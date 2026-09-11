//	The lookat trigger's fires and isLookingAt's answer, one logPrint each,
//	for the A/B in crates/server/tests/sd_plant_ab.rs. Run by
//	tools/run_probe.sh with the --probe-plant / --probe-defuse pair on the
//	server; the client halves write their own fixtures.

main()
{
	thread watch_lookats();
	thread watch_teleports();
	thread watch_bomb_defenders();
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

//	The defuse half has to be at the charge before the 60 s fuse blows it, and
//	a walk across the courtyard does not get there: the flak88 and the cart
//	sit around the zone and a first run circled them for 90 s, reaching the
//	bomb 27 s after it had exploded. So once the plant has spawned the bomb
//	model, every alive defender is put just behind where the planter stood,
//	which is ground a player demonstrably fits on. Same cvar gate as the spawn
//	teleport, and once per level.
watch_bomb_defenders()
{
	if (getcvar("probe_teleport") != "1")
		return;
	if (getcvar("mapname") != "mp_carentan")
		return;

	for (;;)
	{
		if (isdefined(level.bombmodel))
			break;
		wait 0.05;
	}
	//	The defuse or the explosion can delete it between the poll and here.
	if (!isdefined(level.bombmodel))
		return;
	bomb = level.bombmodel.origin;

	planter = find_planter(bomb);
	if (!isdefined(planter))
	{
		logPrint("PROBE teleport_defender none\n");
		return;
	}
	//	24 units back along the planter's own approach line, so the defender
	//	lands on the open ground it came in over rather than inside the charge.
	dest = planter.origin + vec_scale(vectornormalize(planter.origin - bomb), 24);

	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		player = players[i];
		player try_teleport_defender(player getEntityNumber(), dest);
	}
}

//	The alive attacker nearest the bomb: whoever planted it.
find_planter(bomb)
{
	best = undefined;
	bestd = 0;
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		player = players[i];
		if (isalive(player) && isdefined(player.pers["team"]) && player.pers["team"] == game["attackers"])
		{
			d = distance(player.origin, bomb);
			if (!isdefined(best) || d < bestd)
			{
				best = player;
				bestd = d;
			}
		}
	}
	return best;
}

try_teleport_defender(num, dest)
{
	if (!isalive(self))
		return;
	if (!isdefined(self.pers["team"]))
		return;
	if (self.pers["team"] == game["attackers"])
		return;

	self setOrigin(dest);
	logPrint("PROBE teleport_defender " + num + " " + dest + "\n");
}

//	maps\mp\_utility::vectorScale inline, so the probe loads no extra file.
vec_scale(v, s)
{
	return (v[0] * s, v[1] * s, v[2] * s);
}
