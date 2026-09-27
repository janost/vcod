//	Whether a player's body stops radiusDamage's CanDamage traces toward a
//	player standing behind it. Four clients; two stand on one line from the
//	blast in the open, the other two are parked out of range. After the two
//	first blasts the front one is turned through eight yaws, one blast each,
//	and last it is killed, so the ninth blast is behind its corpse.

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
		logPrint("PROBE place " + players[i] getEntityNumber() + " " + spots[i] + " " + players[i].angles + "\n");
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
	//	The front body's facing decides which probes its bones cross. The
	//	1.5 s lets the legs settle on the new yaw; the origins are logged
	//	because the client's periodic nudge can walk it off the station.
	for (j = 0; j < 8; j++)
	{
		yaw = j * 45;
		players[0].health = 100;
		players[1].health = 100;
		players[0] setorigin(spots[0]);
		players[0] setPlayerAngles((0, yaw, 0));
		wait 1.5;
		logPrint("PROBE blast yaw " + yaw + " " + players[0].angles + " " + players[0].origin + " " + players[1].origin + "\n");
		radiusDamage(blast, 500, 20, 20);
		wait 1;
	}
	players[0] setorigin(spots[0]);
	wait 1;
	players[0] suicide();
	wait 1.5;
	logPrint("PROBE blast corpse " + players[0].sessionstate + " " + players[1].origin + "\n");
	radiusDamage(blast, 500, 20, 20);
	wait 1;
	logPrint("PROBE done\n");
}
