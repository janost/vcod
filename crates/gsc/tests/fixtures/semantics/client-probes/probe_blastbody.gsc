//	Whether a player's body stops radiusDamage's CanDamage traces toward a
//	player standing behind it. Four clients; two stand on one line from the
//	blast in the open, the other two are parked out of range.

main()
{
	thread drive();
	maps\mp\gametypes\sd::main();
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

	blast = (-176.8, 2473.1, 7);
	spots[0] = (-226, 2424, -32);
	spots[1] = (-269, 2381, -32);
	spots[2] = (400, 3272, -23.875);
	spots[3] = (224, -1280, 1.86);
	players = getentarray("player", "classname");
	for (i = 0; i < players.size && i < spots.size; i++)
	{
		players[i] setorigin(spots[i]);
		logPrint("PROBE place " + players[i] getEntityNumber() + " " + spots[i] + "\n");
	}
	wait 1;
	logPrint("PROBE blast shielded\n");
	radiusDamage(blast, 500, 20, 20);
	wait 1;
	//	The front player out of the line: the same blast on the back one alone.
	players[0] setorigin((-300, 2473, -24));
	wait 1;
	logPrint("PROBE blast unshielded " + players[0].origin + " " + players[1].origin + "\n");
	radiusDamage(blast, 500, 20, 20);
	wait 1;
	logPrint("PROBE done\n");
}
