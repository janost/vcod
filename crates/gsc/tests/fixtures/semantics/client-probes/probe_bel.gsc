//	Behind Enemy Lines' team swap, for the A/B in
//	crates/server/tests/gametypes_ab.rs. Runs stock bel.gsc on mp_brecourt
//	with two --save-scripted probes that both answer the team menu with axis;
//	bel.gsc moves one of them to allies. The kills are this script's: the
//	victim takes finishPlayerDamage from the other player, which reaches
//	bel.gsc's Callback_PlayerKilled with that player as the attacker the way
//	a rifle round would. Recipe and measurements:
//	docs/research/cod11-gametypes-re-bel.md, 8.
//
//	Kill 1: the axis player kills the allied one (the swap). Kill 2: the same
//	again the other way round. Kill 3: the allied player kills the axis one
//	(a point, no swap).

main()
{
	thread choreograph();
	maps\mp\gametypes\bel::main();
}

choreograph()
{
	if (getcvar("mapname") != "mp_brecourt")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	level thread watch_state();
	level thread place_players();

	for (n = 1; n <= 3; n++)
	{
		allied = wait_player("allies");
		axis = wait_player("axis");
		wait 3;
		if (n < 3)
		{
			victim = allied;
			attacker = axis;
		}
		else
		{
			victim = axis;
			attacker = allied;
		}
		logPrint("PROBE kill " + n + " " + getTime() + " " + victim getEntityNumber() + " by " + attacker getEntityNumber() + "\n");
		victim finishPlayerDamage(attacker, attacker, 1000, 0, "MOD_RIFLE_BULLET", attacker getCurrentWeapon(), victim.origin, (1, 0, 0), "torso_upper");
		//	The swap and the respawns it starts: the killcam, the blackscreen,
		//	the weapon menu or its 6 s timeout.
		wait 30;
	}
	logPrint("PROBE done " + getTime() + "\n");
}

wait_player(team)
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (isdefined(p.pers["team"]) && p.pers["team"] == team && p.sessionstate == "playing" && isalive(p))
				return p;
		}
		wait 0.05;
	}
}

//	Every spawn into play is moved to a fixed spot by team, 88 units apart on
//	open ground and facing each other, so where the bodies and the corpses
//	land does not ride on bel.gsc's random spawn pick. The allied compass
//	marker still starts where the spawn put the player: make_obj_marker reads
//	the origin inside spawnPlayer, before this runs.
place_players()
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (!isdefined(players[i].probe_watch))
			{
				players[i].probe_watch = true;
				players[i] thread place_on_spawn();
			}
		}
		wait 0.05;
	}
}

//	spawnPlayer, spawnSpectator and spawnIntermission all open on
//	notify("spawned"); a frame later the session state says which it was.
place_on_spawn()
{
	for (;;)
	{
		self waittill("spawned");
		wait 0.05;
		if (self.sessionstate != "playing")
			continue;
		if (self.pers["team"] == "allies")
		{
			self setOrigin((1432, -988, -37));
			self setPlayerAngles((0, 90, 0));
		}
		else
		{
			self setOrigin((1432, -900, -39));
			self setPlayerAngles((0, 270, 0));
		}
		logPrint("PROBE placed " + self getEntityNumber() + " " + self.pers["team"] + " " + getTime() + "\n");
	}
}

//	One line whenever a watched value moves: per player, the team, the
//	session state, the score fields and the weapon it holds.
watch_state()
{
	last = "";
	for (;;)
	{
		s = "";
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			s = s + " | " + p getEntityNumber() + " " + str(p.pers["team"]) + " " + str(p.sessionteam) + " " + str(p.sessionstate) + " st=" + str(p.statusicon) + " hi=" + str(p.headicon) + " ht=" + str(p.headiconteam) + " sc=" + p.score + " d=" + p.deaths;
			if (p.sessionstate == "playing")
				s = s + " w=" + p getCurrentWeapon();
		}
		if (s != last)
		{
			logPrint("PROBE state" + s + "\n");
			last = s;
		}
		wait 0.05;
	}
}

str(v)
{
	if (!isdefined(v))
		return "undef";
	return v;
}
