//	The area-tree order a mover's push leaves its players in (movers doc 12,
//	combat doc 14.7), read off a flat radiusDamage's callback order.
//	mp_carentan, dm rules, two clients. Keeps the bombzone_A brush model *5
//	(probe_ride's slab), stands both players on the ground in its path,
//	setorigin first one then the other, then pushes them with one-frame and
//	two-frame movey's and blasts after each. Every frame of a push logs both
//	origins. Recipe: README.md.

main()
{
	bms = getentarray("script_brushmodel", "classname");
	for (i = 0; i < bms.size; i++)
		bms[i].script_gameobjectname = "dm";
	thread drive();
	maps\mp\gametypes\dm::main();
}

wrapDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE cb " + self getEntityNumber() + " " + iDamage + "\n");
	self.health = 1000;
}

state(players, tag)
{
	for (i = 0; i < players.size; i++)
		logPrint("PROBE " + tag + " " + players[i] getEntityNumber() + " " + players[i].origin + "\n");
}

blast(players, tag)
{
	wait 1;
	state(players, tag);
	logPrint("PROBE blast " + tag + "\n");
	radiusDamage((-176.8, 2473.1, 7), 500, 20, 20);
}

push(players, bz, tag, dy, secs)
{
	logPrint("PROBE push " + tag + " " + getTime() + "\n");
	bz movey(dy, secs);
	for (i = 0; i < 4; i++)
	{
		state(players, "f_" + tag + "_" + getTime());
		wait 0.05;
	}
	blast(players, tag);
}

drive()
{
	bms = getentarray("script_brushmodel", "classname");
	bz = undefined;
	for (i = 0; i < bms.size; i++)
	{
		if (!isdefined(bz) || bms[i] getEntityNumber() < bz getEntityNumber())
			bz = bms[i];
	}
	for (;;)
	{
		players = getentarray("player", "classname");
		n = 0;
		for (i = 0; i < players.size; i++)
		{
			if (isdefined(players[i].sessionstate) && players[i].sessionstate == "playing")
				n++;
		}
		if (n >= 2)
			break;
		wait 0.05;
	}
	wait 2;
	level.callbackPlayerDamage = ::wrapDamage;
	logPrint("PROBE bz " + bz getEntityNumber() + "\n");

	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
		players[i].health = 1000;

	//	Slot order, so the second is at its node's head.
	players[0] setorigin((-250, 2430, -31));
	players[1] setorigin((-215, 2430, -31));
	blast(players, "placed");

	push(players, bz, "one", -8, 0.05);
	push(players, bz, "two", -8, 0.1);
	push(players, bz, "three", -12, 0.15);
	push(players, bz, "back", 28, 0.1);

	wait 1;
	logPrint("PROBE done\n");
}
