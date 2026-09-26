//	Who the S&D charge's blast reaches. Runs sd as the gametype, and once four
//	clients are playing puts each on a fixed spot round the charge the
//	committed plant capture's getPlant gave, on mp_carentan's flak88 beside
//	bombzone_A. Four radiusDamage calls at a flat 20 come first, with the bomb
//	model standing; then the tail of sd.gsc's plant success branch from the
//	charge's spawn on, and the stock bomb_countdown runs out. Each client's
//	health and sessionstate are logged before the fuse and the frame after it;
//	sd's own D;/K; records carry the damage. Recipe: README.md.

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

	//	The spot getPlant gave for the committed plant capture's own planter.
	bomb = (-176.8, 2473.1, -22.956871);
	//	One in the open 25 units off, three on the flak88's far side.
	spots[0] = (-196, 2455, -22);
	spots[1] = (-84, 2511, -22);
	spots[2] = (-112, 2446, -22);
	spots[3] = (-77, 2473, -32);

	players = getentarray("player", "classname");
	for (i = 0; i < players.size && i < spots.size; i++)
	{
		players[i] setorigin(spots[i]);
		logPrint("PROBE place " + players[i] getEntityNumber() + " " + spots[i] + "\n");
	}

	level.bombexploder = "2";
	bombzone_A = getent("bombzone_A", "targetname");
	bombzone_B = getent("bombzone_B", "targetname");
	bombzone_A delete();
	bombzone_B delete();
	objective_delete(0);
	objective_delete(1);

	level.bombmodel = spawn("script_model", bomb);
	level.bombmodel.angles = (0, 0, 0);
	level.bombmodel setmodel("xmodel/mp_bomb1_defuse");
	level.bombmodel playSound("Explo_plant_no_tick");

	bombtrigger = getent("bombtrigger", "targetname");
	bombtrigger.origin = level.bombmodel.origin;
	level.bombplanted = true;
	logPrint("PROBE bomb " + bomb + "\n");
	logPrint("PROBE trigger getorigin " + bombtrigger getorigin() + " origin " + bombtrigger.origin + "\n");

	wait 1;
	logPrint("PROBE blast at_bomb\n");
	radiusDamage(bomb, 500, 20, 20);
	wait 1;
	logPrint("PROBE blast above_bomb\n");
	radiusDamage(bomb + (0, 0, 20), 500, 20, 20);
	wait 1;
	logPrint("PROBE blast trigger_getorigin\n");
	radiusDamage(bombtrigger getorigin(), 500, 20, 20);
	wait 1;
	logPrint("PROBE blast above_player0\n");
	radiusDamage(players[0].origin + (0, 0, 40), 500, 20, 20);
	wait 1;
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		players[i].health = 100;
		logPrint("PROBE before " + p getEntityNumber() + " " + p.health + " " + p.sessionstate + " " + p.origin + "\n");
	}

	bombtrigger thread maps\mp\gametypes\sd::bomb_think();
	bombtrigger thread maps\mp\gametypes\sd::bomb_countdown();

	while (!level.bombexploded)
		wait 0.05;
	wait 0.05;
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		logPrint("PROBE after " + p getEntityNumber() + " " + p.health + " " + p.sessionstate + " " + p.origin + "\n");
	}
}
