//	Which links a grenade's blast walk meets (combat doc 14.7, "One entity
//	pass"). dm rules. A script_origin parent moves 8 units up or down every
//	frame, so it stays put on average; the map's spawn points (numbered
//	below any grenade) hang off it from the start, and 8 spawned
//	script_origins (numbered above the first grenade) once a grenade is
//	live. The first player to spawn is the victim, held at 1000 health and
//	glued 60 over each grenade in flight; the second is the --save-grenade
//	thrower, parked out of reach while a grenade is live. On each hit the
//	victim's callback logs, per child, its number, the grenade's and how far
//	the child is off the gap it linked at: 0 when the entity pass
//	re-anchored it before the blast, 8 either way when it still sits on the
//	last frame's anchor. Recipe: README.md.

main()
{
	thread drive();
	maps\mp\gametypes\dm::main();
}

hit(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	g = -1;
	if (isdefined(eInflictor))
		g = eInflictor getEntityNumber();
	for (i = 0; i < level.kids.size; i++)
	{
		k = level.kids[i];
		logPrint("PROBE link " + gettime() + " " + sMeansOfDeath + " " + g + " " + k getEntityNumber() + " " + (level.p.origin[2] - k.origin[2] - k.gap) + "\n");
	}
}

hang(k)
{
	k linkto(level.p);
	k.gap = level.p.origin[2] - k.origin[2];
	level.kids[level.kids.size] = k;
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
	level.p = spawn("script_origin", (-176.8, 2473.1, 400));
	level.kids = [];
	spots = getentarray("mp_deathmatch_spawn", "classname");
	for (i = 0; i < spots.size; i++)
	{
		spots[i] enablelinkto();
		hang(spots[i]);
	}
	victim = undefined;
	thrower = undefined;
	while (!isdefined(thrower))
	{
		players = playing();
		for (i = 0; i < players.size; i++)
		{
			if (!isdefined(victim))
				victim = players[i];
			else if (players[i] != victim)
				thrower = players[i];
		}
		wait 0.05;
	}
	level.callbackPlayerDamage = ::hit;
	logPrint("PROBE roles " + victim getEntityNumber() + " " + thrower getEntityNumber() + "\n");
	spot = (-176.8, 2473.1, -32);
	park = (400, 3272, -23.875);
	thrower setorigin(spot);
	victim setorigin(park);
	away = 0;
	spawned = 0;
	up = 8;
	for (;;)
	{
		level.p.origin = level.p.origin + (0, 0, up);
		up = 0 - up;
		if (!isdefined(thrower) || !isdefined(victim))
			return;
		thrower.health = 1000;
		victim.health = 1000;
		grenades = getentarray("grenade", "classname");
		if (grenades.size > 0)
		{
			if (!spawned)
			{
				spawned = 1;
				logPrint("PROBE grenade " + grenades[0] getEntityNumber() + "\n");
				for (i = 0; i < 8; i++)
					hang(spawn("script_origin", level.p.origin - (0, 0, 100)));
			}
			if (!away)
			{
				thrower setorigin(park);
				away = 1;
			}
			victim setorigin(grenades[0].origin + (0, 0, 60));
		}
		else if (away)
		{
			thrower setorigin(spot);
			victim setorigin(park);
			away = 0;
		}
		wait 0.05;
	}
}
