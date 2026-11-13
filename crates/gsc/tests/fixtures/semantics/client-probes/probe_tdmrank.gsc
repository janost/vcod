//	United Offensive's battle rank (_rank_gmi) climbed from rank 1 to 5 under
//	stock UO tdm.gsc on mp_cassino, for the A/B in
//	crates/server/tests/gametypes_ab.rs. Two --save-scripted probes, allies
//	(attacker) then axis (defender). scr_rank_ppr 2 puts the ranks at 2, 4, 6
//	and 8 points; the killcam is off and scr_forcerespawn 1 respawns a dead
//	player on its own. The attacker kills the defender twice per rank through
//	finishPlayerDamage, then the defender kills him so he respawns with that
//	rank's loadout, which the probe logs slot by slot. At rank 5 the
//	artillery binoculars get their round after scr_artillery_first_interval
//	(3 s here). Then a live demotion (the probe sets his score to 5), and the
//	time limit ends the map; the restart exitLevel(false) falls back to shows
//	the rank after a map end. Recipe and measurements:
//	docs/research/coduo-diff.md 22.

main()
{
	setCvar("scr_tdm_timelimit", "1.5");
	setCvar("scr_tdm_scorelimit", "0");
	setCvar("scr_rank_ppr", "2");
	setCvar("scr_killcam", "0");
	setCvar("scr_forcerespawn", "1");
	setCvar("scr_artillery_first_interval", "3");
	thread choreograph();
	level thread place_players();
	maps\mp\gametypes\tdm::main();
}

choreograph()
{
	if (getcvar("mapname") != "mp_cassino")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	level thread watch_state();
	// The level exitLevel(false) restarts into: who spawns at which rank.
	if (getcvar("probe_ended") == "1")
	{
		attacker = wait_player("allies");
		wait 1;
		attacker loadout("after map end");
		logPrint("PROBE done " + getTime() + "\n");
		return;
	}
	attacker = wait_player("allies");
	defender = wait_player("axis");
	logPrint("PROBE players " + attacker getEntityNumber() + " " + defender getEntityNumber() + "\n");
	wait 2;
	attacker loadout("start");
	defender loadout("start");

	for (rank = 2; rank <= 5; rank++)
	{
		for (k = 0; k < 2; k++)
		{
			defender = wait_player("axis");
			wait 0.5;
			defender finishPlayerDamage(attacker, attacker, 1000, 0, "MOD_RIFLE_BULLET", attacker getCurrentWeapon(), defender.origin, (1, 0, 0), "torso_upper");
			logPrint("PROBE killed " + getTime() + " " + attacker.score + "\n");
		}
		// updateGametypeCvars checks for rank changes once a second.
		wait 1.5;
		attacker ranks("alive at " + rank);
		defender = wait_player("axis");
		wait 0.5;
		attacker finishPlayerDamage(defender, defender, 1000, 0, "MOD_RIFLE_BULLET", defender getCurrentWeapon(), attacker.origin, (-1, 0, 0), "torso_upper");
		logPrint("PROBE died " + getTime() + "\n");
		attacker = wait_player("allies");
		wait 1;
		attacker loadout("spawned at " + rank);
	}

	// dispense_artillery_strike: the round after the first interval.
	wait 3;
	attacker loadout("artillery");

	// A live demotion: rank 5 to 3.
	attacker.score = 5;
	wait 1.5;
	attacker ranks("demoted");

	while (game["state"] != "intermission")
		wait 0.05;
	logPrint("PROBE intermission " + getTime() + "\n");
	setCvar("probe_ended", "1");
}

//	The rank's script state and the icons it set.
ranks(tag)
{
	logPrint("PROBE rank " + tag + " " + self getEntityNumber() + " sc=" + self.score + " rank=" + str(self.pers["rank"]) + " st=" + str(self.statusicon) + " head=" + str(self.headicon) + " " + str(self.headiconteam) + "\n");
}

//	Every weapon slot with its weapon, reserve and clip.
loadout(tag)
{
	self ranks(tag);
	slots[0] = "primary";
	slots[1] = "primaryb";
	slots[2] = "pistol";
	slots[3] = "grenade";
	slots[4] = "smokegrenade";
	slots[5] = "satchel";
	slots[6] = "binocular";
	s = "";
	for (i = 0; i < slots.size; i++)
		s = s + " " + slots[i] + "=" + self getWeaponSlotWeapon(slots[i]) + ":" + self getWeaponSlotAmmo(slots[i]) + ":" + self getWeaponSlotClipAmmo(slots[i]);
	logPrint("PROBE loadout " + tag + s + "\n");
}

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

//	One line whenever a watched value moves: the players' state, score,
//	rank and icons, the team scores and the game state.
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
			s = s + " | " + p getEntityNumber() + " " + p.sessionstate + " sc=" + p.score + " d=" + p.deaths + " rank=" + str(p.pers["rank"]) + " st=" + str(p.statusicon) + " head=" + str(p.headicon);
		}
		s = s + " | allies " + getTeamScore("allies") + " axis " + getTeamScore("axis") + " " + str(game["state"]);
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

//	Every spawn is moved onto a fixed spot per team, so what each probe sees
//	of the other is not the spawn script's random pick.
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

place_on_spawn()
{
	for (;;)
	{
		self waittill("spawned");
		wait 0.05;
		if (self.sessionstate != "playing")
			continue;
		self setOrigin(home(self.pers["team"]));
		self setPlayerAngles((0, 0, 0));
		// Who answers the team menu first after the restart is the two
		// clients' race, so the restarted level logs no placements.
		if (getcvar("probe_ended") != "1")
			logPrint("PROBE placed " + self getEntityNumber() + " " + self.pers["team"] + " " + self.origin + " " + self.angles + "\n");
	}
}

home(team)
{
	return getentarray("mp_uo_spawn_" + team, "classname")[0].origin;
}
