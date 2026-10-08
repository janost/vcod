//	What a blast victim's damage callback does to the victims the walk has
//	not reached yet (combat doc 14.5). dm rules, 1000 health, every hit
//	logged and none landed but where a row kills on purpose. The first
//	three players run the radiusDamage rows; the fourth to spawn is the
//	--save-grenade thrower, and every frame after that the first three stand
//	glued round its grenade in flight (round the thrower between throws) so
//	each grenade's walk has them in reach: the first victim of each grenade
//	walk parks the other two. Recipe: README.md.

main()
{
	thread drive();
	maps\mp\gametypes\dm::main();
}

wrapDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	n = self getEntityNumber();
	if (sMeansOfDeath == "MOD_GRENADE_SPLASH")
	{
		grenade_hit(n, iDamage);
		return;
	}
	logPrint("PROBE cb " + level.probe_row + " " + n + " " + iDamage + " " + self.sessionstate + " " + self.origin + "\n");
	if (level.probe_mode == "kill_inner")
	{
		self.health = 1;
		[[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
		return;
	}
	if (level.probe_first)
		return;
	level.probe_first = 1;
	mode = level.probe_mode;
	others = level.probe_players;
	if (mode == "park")
	{
		for (i = 0; i < others.size; i++)
		{
			if (others[i] != self)
				others[i] setorigin(level.probe_park[i]);
		}
	}
	else if (mode == "pull")
		level.probe_parked setorigin((-200, 2430, -32));
	else if (mode == "ignore")
		setPlayerIgnoreRadiusDamage(true);
	else if (mode == "nested")
	{
		for (i = 0; i < others.size; i++)
		{
			if (others[i] != self)
			{
				level.probe_row = "nested_inner";
				radiusDamage(others[i].origin + (0, 0, 30), 40, 5, 5);
				level.probe_row = "nested";
				break;
			}
		}
	}
	else if (mode == "kill")
	{
		level.probe_mode = "kill_inner";
		level.probe_row = "kill_inner";
		for (i = 0; i < others.size; i++)
		{
			if (others[i] != self)
				radiusDamage(others[i].origin + (0, 0, 30), 40, 50, 50);
		}
		level.probe_mode = "kill";
		level.probe_row = "kill";
	}
	logPrint("PROBE cbdone " + level.probe_row + " " + n + "\n");
}

//	The first victim of a grenade's walk parks the other two; any later
//	victim of the same walk logs where it was met.
grenade_hit(n, iDamage)
{
	if (!isdefined(level.probe_gwalk) || level.probe_gwalk != gettime())
	{
		level.probe_gwalk = gettime();
		logPrint("PROBE gcb first " + gettime() + " " + n + " " + iDamage + " " + self.origin + "\n");
		state("gwalk");
		others = level.probe_players;
		for (i = 0; i < others.size; i++)
		{
			if (others[i] != self)
				others[i] setorigin(level.probe_park[i]);
		}
		return;
	}
	logPrint("PROBE gcb later " + gettime() + " " + n + " " + iDamage + " " + self.origin + "\n");
}

state(tag)
{
	players = level.probe_players;
	for (i = 0; i < players.size; i++)
		logPrint("PROBE " + tag + " " + players[i] getEntityNumber() + " " + players[i].sessionstate + " " + players[i].origin + "\n");
}

place()
{
	spots[0] = (-290, 2430, -32);
	spots[1] = (-260, 2480, -32);
	spots[2] = (-230, 2380, -32);
	players = level.probe_players;
	for (i = 0; i < players.size; i++)
	{
		players[i].health = 1000;
		players[i] setorigin(spots[i]);
	}
	wait 1;
}

blast(row, mode)
{
	level.probe_row = row;
	level.probe_mode = mode;
	level.probe_first = 0;
	state(row);
	logPrint("PROBE blast " + row + "\n");
	radiusDamage((-176.8, 2473.1, 7), 500, 20, 20);
	logPrint("PROBE blastdone " + row + "\n");
	wait 0.5;
}

playing()
{
	out = [];
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		if (players[i].sessionstate == "playing")
			out[out.size] = players[i];
	}
	return out;
}

drive()
{
	wait 0.05;
	for (;;)
	{
		if (playing().size >= 3)
			break;
		wait 0.05;
	}
	wait 2;
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::wrapDamage;
	level.probe_players = playing();
	level.probe_park[0] = (400, 3272, -23.875);
	level.probe_park[1] = (224, -1280, 1.86);
	level.probe_park[2] = (-1100, 1500, 0);

	place();
	blast("plain", "none");

	place();
	blast("move_out", "park");

	place();
	level.probe_parked = level.probe_players[2];
	level.probe_parked setorigin(level.probe_park[2]);
	wait 0.5;
	blast("move_in", "pull");

	place();
	blast("ignore_mid", "ignore");
	blast("ignore_after", "none");
	setPlayerIgnoreRadiusDamage(false);

	place();
	blast("nested", "nested");

	place();
	blast("kill", "kill");
	wait 0.05;
	state("kill_next_frame");
	logPrint("PROBE rows done\n");

	// The grenade half: the fourth player to spawn throws.
	thrower = undefined;
	while (!isdefined(thrower))
	{
		players = playing();
		for (i = 0; i < players.size; i++)
		{
			if (players[i] != level.probe_players[0] && players[i] != level.probe_players[1] && players[i] != level.probe_players[2])
				thrower = players[i];
		}
		wait 0.05;
	}
	logPrint("PROBE thrower " + thrower getEntityNumber() + "\n");
	thrower setorigin((-176.8, 2473.1, -32));
	for (;;)
	{
		// Round the grenade in flight, else round the thrower.
		if (!isdefined(thrower))
			return;
		at = thrower.origin;
		grenades = getentarray("grenade", "classname");
		if (grenades.size > 0)
			at = grenades[0].origin;
		offs[0] = (60, 0, 0);
		offs[1] = (0, 60, 0);
		offs[2] = (-60, 0, 0);
		players = level.probe_players;
		for (i = 0; i < players.size; i++)
		{
			if (players[i].sessionstate != "playing")
				continue;
			players[i].health = 1000;
			players[i] setorigin(at + offs[i]);
		}
		wait 0.05;
	}
}
