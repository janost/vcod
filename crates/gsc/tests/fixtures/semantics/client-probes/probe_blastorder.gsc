//	The order one radiusDamage walks its victims in, which is the engine's
//	area tree (combat doc 14.7). Four clients, sd rules, 1000 health each so
//	nobody dies. Each blast is a flat 20 from one point with every player in
//	the open around it, so every callback runs and the cb lines read the
//	walk. Between blasts one player is set down somewhere else: within the
//	node it already sits in (where a relink keeps its place), into that node
//	from another, and out of it. Recipe: README.md.

main()
{
	thread drive();
	maps\mp\gametypes\sd::main();
}

wrapDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE cb " + self getEntityNumber() + " " + iDamage + "\n");
	self.health = 1000;
}

state(players, tag)
{
	for (i = 0; i < players.size; i++)
		logPrint("PROBE " + tag + " " + players[i] getEntityNumber() + " " + players[i].sessionstate + " " + players[i].origin + "\n");
}

blast(players, tag)
{
	wait 1;
	state(players, tag);
	logPrint("PROBE blast " + tag + "\n");
	radiusDamage((-176.8, 2473.1, 7), 500, 20, 20);
}

drive()
{
	wait 0.05;
	if (!game["matchstarted"])
		return;
	for (;;)
	{
		players = getentarray("player", "classname");
		n = 0;
		for (i = 0; i < players.size; i++)
		{
			if (players[i].sessionstate == "playing")
				n++;
		}
		if (n >= 4)
			break;
		wait 0.05;
	}
	wait 2;
	level.callbackPlayerDamage = ::wrapDamage;

	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
		players[i].health = 1000;

	// In slot order: two across the second split, one below it, one above.
	players[0] setorigin((-290, 2430, -32));
	players[1] setorigin((-260, 2480, -32));
	players[2] setorigin((-230, 2380, -32));
	players[3] setorigin((-230, 2540, -32));
	blast(players, "placed");

	// Ten units along, still across the split.
	players[0] setorigin((-290, 2440, -32));
	blast(players, "moved_within");

	// From below the split to across it.
	players[2] setorigin((-200, 2430, -32));
	blast(players, "moved_into");

	// From across the split to below it.
	players[1] setorigin((-290, 2380, -32));
	blast(players, "moved_out");

	wait 1;
	logPrint("PROBE done\n");
}
