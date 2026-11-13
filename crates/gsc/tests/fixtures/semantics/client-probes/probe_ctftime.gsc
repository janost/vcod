//	United Offensive's Capture the Flag returned by a defender's touch, its
//	round and map time limits, and a rotation naming another map, for the
//	A/B in crates/server/tests/gametypes_ab.rs. Runs stock UO ctf.gsc on
//	mp_cassino with two --save-scripted probes, allies then axis, under a
//	half-minute round and a half-minute map time limit, no score or round
//	limit, and a rotation naming mp_foy that the probe writes itself. The
//	first level ends in ctf's own match-start restart. On the second the
//	allied player picks up the axis flag and kills himself on it, and the
//	axis player is put on the dropped flag, which returns it. The round
//	runs out (a draw), the map time limit is past at its end, the map ends
//	in intermission and exitLevel(false) rotates to mp_foy. Recipe and
//	measurements: docs/research/coduo-diff.md 22.

main()
{
	if (getcvar("probe_ended") != "1")
	{
		setCvar("scr_ctf_scorelimit", "0");
		setCvar("scr_ctf_roundlimit", "0");
		setCvar("scr_ctf_roundlength", "0.5");
		setCvar("scr_ctf_timelimit", "0.5");
		setCvar("scr_ctf_startrounddelay", "5");
		setCvar("sv_mapRotation", "gametype probe_ctftime map mp_foy");
	}
	thread choreograph();
	level thread place_players();
	maps\mp\gametypes\ctf::main();
}

choreograph()
{
	// The level the rotation loads.
	if (getcvar("probe_ended") == "1")
	{
		logPrint("PROBE done " + getcvar("mapname") + " " + getcvar("g_gametype") + " " + getTime() + "\n");
		level thread watch_state();
		return;
	}
	if (getcvar("mapname") != "mp_cassino")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	if (!game["matchstarted"])
	{
		logPrint("PROBE prematch\n");
		return;
	}
	level thread watch_state();
	attacker = wait_player("allies");
	defender = wait_player("axis");
	logPrint("PROBE players " + attacker getEntityNumber() + " " + defender getEntityNumber() + "\n");
	wait 2;

	flag = level.axis_flag;
	attacker setOrigin(flag.trigger.origin + (0, 0, 8));
	logPrint("PROBE to_flag " + getTime() + " " + attacker.origin + "\n");
	for (t = 0; t < 10 && !isdefined(attacker.hasflag); t += 0.05)
		wait 0.05;
	logPrint("PROBE picked_up " + getTime() + " " + isdefined(attacker.hasflag) + "\n");
	wait 1;

	// The flag drops 11 units ahead of where the body faces (getPlant).
	logPrint("PROBE facing " + getTime() + " " + attacker.angles + "\n");
	attacker suicide();
	logPrint("PROBE suicide " + getTime() + "\n");
	wait 1;
	logPrint("PROBE dropped " + getTime() + " " + flag.moved + " " + flag.mobile_trigger.origin + "\n");
	defender setOrigin(flag.mobile_trigger.origin + (0, 0, 8));
	logPrint("PROBE to_drop " + getTime() + " " + defender.origin + "\n");
	for (t = 0; t < 10 && flag.moved; t += 0.05)
		wait 0.05;
	logPrint("PROBE returned " + getTime() + " " + flag.moved + "\n");

	defender setOrigin(home("axis"));
	while (game["state"] != "intermission")
		wait 0.05;
	logPrint("PROBE intermission " + getTime() + " " + game["timepassed"] + "\n");
	setCvar("probe_ended", "1");
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

//	One line whenever a watched value moves: both flags and their carrier,
//	the players' state, carrier field and score, the team scores and the
//	game state.
watch_state()
{
	last = "";
	for (;;)
	{
		s = "allies_flag " + flag_str(level.allies_flag) + " axis_flag " + flag_str(level.axis_flag);
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			s = s + " | " + p getEntityNumber() + " " + p.sessionstate + " st=" + str(p.statusicon) + " sc=" + p.score + " flag=" + isdefined(p.hasflag);
		}
		s = s + " | allies " + getTeamScore("allies") + " axis " + getTeamScore("axis") + " " + str(game["state"]) + " ended=" + str(level.roundended) + " map=" + str(level.mapended);
		if (s != last)
		{
			logPrint("PROBE state " + s + "\n");
			last = s;
		}
		wait 0.05;
	}
}

flag_str(f)
{
	if (!isdefined(f))
		return "undef";
	s = "moved=" + f.moved;
	if (isdefined(f.carried_by))
		s = s + " by=" + f.carried_by getEntityNumber();
	return s;
}

str(v)
{
	if (!isdefined(v))
		return "undef";
	return v;
}

//	Every spawn is moved onto a fixed spot per team, so what each probe sees
//	of the other is not the spawn script's random pick. A player already
//	playing when his watcher starts is placed at once: ctf's match-start
//	map_restart spawns him in the level's first frame, before anyone waits
//	on "spawned", and he would keep the spawn point's facing.
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
	if (self.sessionstate == "playing")
		self place();
	for (;;)
	{
		self waittill("spawned");
		wait 0.05;
		if (self.sessionstate == "playing")
			self place();
	}
}

place()
{
	self setOrigin(home(self.pers["team"]));
	self setPlayerAngles((0, 0, 0));
	logPrint("PROBE placed " + self getEntityNumber() + " " + self.pers["team"] + " " + self.origin + " " + self.angles + "\n");
	wait 0.5;
	logPrint("PROBE settled " + self getEntityNumber() + " " + self.angles + "\n");
}

home(team)
{
	return getentarray("mp_uo_spawn_" + team, "classname")[0].origin;
}
