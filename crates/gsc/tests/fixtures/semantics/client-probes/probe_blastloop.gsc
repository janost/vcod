//	What one radiusDamage does between its victims, and who it reaches.
//	Four clients, sd rules. The damage callback is wrapped to log every
//	victim in the order the engine calls it. First the player-ignore flag
//	across two blasts. Then two lethal blasts down a line, each with a
//	100-health player in front and a 1000-health one behind: once with the
//	lower slot in front, once with the higher. Each is followed by a second
//	blast in the same frame and a third a frame later, which ask whether the
//	player the first one killed is still a candidate. Recipe: README.md.

main()
{
	thread drive();
	maps\mp\gametypes\sd::main();
}

wrapDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	n = self getEntityNumber();
	logPrint("PROBE cb " + n + " " + iDamage + " " + self.sessionstate + " " + self.health + "\n");
	[[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
	logPrint("PROBE cbafter " + n + " " + self.sessionstate + " " + self.health + "\n");
}

state(players, tag)
{
	for (i = 0; i < players.size; i++)
		logPrint("PROBE " + tag + " " + i + " " + players[i].sessionstate + " " + players[i].health + " " + players[i].origin + "\n");
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
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::wrapDamage;

	blast = (-176.8, 2473.1, 7);
	front = (-226, 2424, -32);
	back = (-269, 2381, -32);
	front2 = (-246.8, 2473.1, -32);
	back2 = (-306.8, 2473.1, -32);
	park2 = (400, 3272, -23.875);
	park3 = (224, -1280, 1.86);
	players = getentarray("player", "classname");
	players[0] setorigin(front);
	players[1] setorigin(back);
	players[2] setorigin(park2);
	players[3] setorigin(park3);
	wait 1;
	state(players, "place");

	logPrint("PROBE blast ignore_on\n");
	setPlayerIgnoreRadiusDamage(true);
	radiusDamage(blast, 500, 20, 20);
	wait 0.5;
	logPrint("PROBE blast ignore_still\n");
	radiusDamage(blast, 500, 20, 20);
	wait 0.5;
	logPrint("PROBE blast ignore_off\n");
	setPlayerIgnoreRadiusDamage(false);
	radiusDamage(blast, 500, 20, 20);
	wait 1;

	players[0].health = 100;
	players[1].health = 1000;
	players[0] setorigin(front);
	players[1] setorigin(back);
	wait 1;
	state(players, "before_low");
	logPrint("PROBE blast lethal_low\n");
	radiusDamage(blast, 500, 200, 200);
	logPrint("PROBE blast same_frame_low\n");
	radiusDamage(blast, 500, 20, 20);
	wait 0.05;
	logPrint("PROBE blast next_frame_low\n");
	radiusDamage(blast, 500, 20, 20);
	wait 1;
	state(players, "after_low");

	players[1] setorigin(park3);
	players[3] setorigin(front2);
	players[2] setorigin(back2);
	players[3].health = 100;
	players[2].health = 1000;
	wait 1;
	state(players, "before_high");
	logPrint("PROBE blast lethal_high\n");
	radiusDamage(blast, 500, 200, 200);
	logPrint("PROBE blast same_frame_high\n");
	radiusDamage(blast, 500, 20, 20);
	wait 0.05;
	logPrint("PROBE blast next_frame_high\n");
	radiusDamage(blast, 500, 20, 20);
	wait 1;
	state(players, "after_high");
	logPrint("PROBE done\n");
}
