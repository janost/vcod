//	Whether `TeleportPlayer` files a released gunner at the head of his
//	area-tree node, which is the order the next radiusDamage walks
//	(combat doc 14.7). probe_blastorder's four players and blast; then
//	slot 1, standing across the split, is mounted on mp_carentan's gun at
//	(1712 1830 8) through `useby`, a blast runs with him at the gun, the
//	gun is deleted (`G_FreeTurret`'s release teleports him back to where he
//	mounted) and a last blast runs. Recipe: README.md, "probe_blastteleport".

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
	guns = getentarray("misc_mg42", "classname");
	gun = undefined;
	for (i = 0; i < guns.size; i++)
		if (!isdefined(gun) || distance(guns[i].origin, (1712, 1830, 8)) < distance(gun.origin, (1712, 1830, 8)))
			gun = guns[i];

	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
		players[i].health = 1000;

	players[0] setorigin((-290, 2430, -32));
	players[1] setorigin((-260, 2480, -32));
	players[2] setorigin((-230, 2380, -32));
	players[3] setorigin((-230, 2540, -32));
	blast(players, "placed");

	// Slot 0 set down again within its node: the head of the list.
	players[0] setorigin((-290, 2440, -32));
	blast(players, "moved_within");

	gun useby(players[1]);
	logPrint("PROBE mounted " + gun getEntityNumber() + "\n");
	blast(players, "mounted");

	gun delete();
	logPrint("PROBE freed\n");
	blast(players, "released");

	wait 1;
	logPrint("PROBE done\n");
}
