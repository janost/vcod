//	Retrieval's pickup, carry, drop, return and capture, for the A/B in
//	crates/server/tests/gametypes_ab.rs. Runs stock re.gsc on mp_brecourt
//	(one objective, one spawn spot) with two --save-scripted probes, one per
//	team. The script places the players and tells each one when to press use
//	through the client cvar `probe_use` (`tap`, `hold` or `0`), so the pickup
//	goes through the engine's use key and the drop through re.gsc's own hold.
//	Recipe and measurements: docs/research/cod11-gametypes-re-bel.md, 7.
//
//	Round 1: pickup, a hold-use drop away from the spot, the 60 s timeout
//	return, a second pickup and a suicide (drop on death, the attackers
//	eliminated). Round 2: the defender's refused pickup, the attacker's pickup
//	and the walk into the goal (capture, round won). Every state change is a
//	logPrint, so the server's games_mp.log is the script half of the evidence.

main()
{
	thread choreograph();
	maps\mp\gametypes\re::main();
}

choreograph()
{
	if (getcvar("mapname") != "mp_brecourt")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	//	retrieval_spawn_objective sets the hint string a frame after main.
	wait 1;
	if (!game["matchstarted"])
	{
		logPrint("PROBE prematch\n");
		return;
	}
	if (!isdefined(game["probe_round"]))
		game["probe_round"] = 0;
	game["probe_round"]++;
	round = game["probe_round"];

	obj = level.retrieval_objective[0];
	logPrint("PROBE round " + round + " obj " + obj getEntityNumber() + " " + obj.origin + " goal " + obj.goal.origin + "\n");
	level thread watch_state(obj);

	attacker = wait_player(game["re_attackers"]);
	defender = wait_player(game["re_defenders"]);
	logPrint("PROBE players " + attacker getEntityNumber() + " " + defender getEntityNumber() + "\n");
	wait 2;

	if (round == 1)
	{
		pickup(attacker, obj);

		//	Away from the spot, so the drop lands somewhere the return can be
		//	seen to undo: a fixed point on the ground 352 units off, since
		//	the attackers' own spawn is a random pick.
		wait 2;
		attacker setOrigin((1432, -988, -37));
		logPrint("PROBE moved " + attacker.origin + "\n");
		wait 1;
		logPrint("PROBE settled " + attacker.origin + " " + attacker.angles + "\n");
		attacker setClientCvar("probe_use", "hold");
		wait_held(attacker, 0, 8);
		attacker setClientCvar("probe_use", "0");
		logPrint("PROBE dropped " + getTime() + " " + obj.origin + "\n");

		//	objective_timeout's 60 s.
		while (distance(obj.origin, obj.startorigin) > 1)
			wait 0.05;
		logPrint("PROBE returned " + getTime() + " " + obj.origin + "\n");

		wait 2;
		pickup(attacker, obj);
		wait 2;
		logPrint("PROBE suicide " + getTime() + "\n");
		attacker suicide();
		return;
	}

	if (round == 2)
	{
		//	re.gsc answers a defender's use with client_print's hudelem.
		home = defender.origin;
		place(defender, obj);
		defender setClientCvar("probe_use", "tap");
		wait 3;
		defender setClientCvar("probe_use", "0");
		defender setOrigin(home);
		logPrint("PROBE defender_back " + getTime() + "\n");
		wait 2;

		pickup(attacker, obj);
		wait 2;
		//	Inside the goal's box, which reaches 60 units under its origin.
		attacker setOrigin(obj.goal.origin - (0, 0, 50));
		logPrint("PROBE to_goal " + getTime() + " " + attacker.origin + "\n");
		return;
	}

	logPrint("PROBE done " + getTime() + "\n");
}

//	The first living player on `team`.
wait_player(team)
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (isdefined(p.pers["team"]) && p.pers["team"] == team && p.sessionstate == "playing")
				return p;
		}
		wait 0.05;
	}
}

//	On the objective's spot, looking straight down at it, which puts the
//	trigger_use dead ahead of the use key's aim inside its reach.
place(player, obj)
{
	player setOrigin(obj.trigger.origin);
	player setPlayerAngles((85, 0, 0));
	logPrint("PROBE placed " + player getEntityNumber() + " " + getTime() + " " + player.origin + "\n");
}

pickup(player, obj)
{
	place(player, obj);
	player setClientCvar("probe_use", "tap");
	wait_held(player, 1, 10);
	player setClientCvar("probe_use", "0");
	logPrint("PROBE picked_up " + getTime() + " " + player.objs_held + "\n");
}

//	Polled rather than a waittill on re.gsc's own notifies: an endon would
//	end this thread, and a notify of ours would reach re.gsc's waiters.
wait_held(player, held, secs)
{
	for (t = 0; t < secs; t += 0.05)
	{
		if (player.objs_held == held)
			return;
		wait 0.05;
	}
	logPrint("PROBE timeout held " + held + "\n");
}

//	One line whenever a watched value moves: the objective's place and the
//	carrier fields re.gsc writes, per player, and the team scores.
watch_state(obj)
{
	last = "";
	for (;;)
	{
		if (isdefined(obj))
			s = "obj " + obj.origin;
		else
			s = "obj gone";
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			s = s + " | " + p getEntityNumber() + " " + p.sessionstate + " st=" + str(p.statusicon) + " hi=" + str(p.headicon) + " ht=" + str(p.headiconteam) + " sc=" + p.score + " d=" + p.deaths;
			if (isdefined(p.objs_held))
				s = s + " held=" + p.objs_held;
		}
		s = s + " | allies " + getTeamScore("allies") + " axis " + getTeamScore("axis");
		if (s != last)
		{
			logPrint("PROBE state " + s + "\n");
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
