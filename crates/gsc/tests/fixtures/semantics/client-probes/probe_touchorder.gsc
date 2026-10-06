//	The order one touch pass (G_TouchTriggers) meets what it touches, which
//	is the engine's area tree (combat doc 14.7). One client, dm rules, on
//	mp_pavlov where the minefield triggers 101 and 102 overlap. Three
//	item_health are spawned on that spot, and the player is set down there
//	for a fifth of a second per phase; every "trigger" on a minefield and
//	every "touch" on an item logs a line while the phase is on. Damage is
//	swallowed, so the mines kill nobody and a health item is never taken.
//	Between phases item a's origin is written: once within its node, once
//	far away and back. With probe_mode ammo, three rifles instead, and the
//	one the walk meets first is the one taken. Recipe: README.md.

main()
{
	level.probe_on = 0;
	thread drive();
	maps\mp\gametypes\dm::main();
}

wrapDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
}

watch_trigger(num)
{
	for (;;)
	{
		self waittill("trigger", other);
		if (level.probe_on)
			logPrint("PROBE f " + num + " " + getTime() + "\n");
	}
}

watch_mine_touch(num)
{
	for (;;)
	{
		self waittill("touch", other);
		if (level.probe_on)
			logPrint("PROBE mt " + num + " " + getTime() + "\n");
	}
}

watch_player_touch()
{
	for (;;)
	{
		self waittill("touch", other);
		if (level.probe_on)
			logPrint("PROBE pt " + other getEntityNumber() + " " + getTime() + "\n");
	}
}

watch_take(name)
{
	self waittill("trigger", other);
	logPrint("PROBE take " + name + " " + getTime() + "\n");
}

//	The touch order read off a side effect inside the cmd rather than off
//	when a woken thread runs: three of the player's own rifle, and a reserve
//	one round short of full. Only the first item the walk meets can be
//	grabbed, and taking it fills the reserve.
ammo(player, at, away)
{
	logPrint("PROBE weapon " + player getWeaponSlotWeapon("primary") + "\n");
	names = [];
	names[0] = "d";
	names[1] = "e";
	names[2] = "f";
	offsets = [];
	offsets[0] = (8, 0, 16);
	offsets[1] = (0, 8, 16);
	offsets[2] = (-8, 0, 16);
	for (i = 0; i < 3; i++)
	{
		item = spawn("mpweapon_mosinnagant", at + offsets[i]);
		item.count = 5;
		item thread watch_take(names[i]);
	}
	wait 1;
	player setWeaponSlotAmmo("primary", 149);
	logPrint("PROBE phase ammo\n");
	player setorigin(at);
	wait 0.5;
	player setorigin(away);
	wait 1;
	logPrint("PROBE done\n");
}

watch_touch(name)
{
	for (;;)
	{
		self waittill("touch", other);
		if (level.probe_on)
			logPrint("PROBE t " + name + " " + getTime() + "\n");
	}
}

phase(player, at, away, tag, items)
{
	for (i = 0; i < items.size; i++)
		logPrint("PROBE " + tag + " item " + items[i].probe_name + " " + items[i] getEntityNumber() + " " + items[i].origin + "\n");
	logPrint("PROBE phase " + tag + "\n");
	player setorigin(at);
	level.probe_on = 1;
	wait 0.2;
	level.probe_on = 0;
	player setorigin(away);
	wait 1;
}

drive()
{
	wait 0.05;
	for (;;)
	{
		players = getentarray("player", "classname");
		if (players.size > 0 && players[0].sessionstate == "playing")
			break;
		wait 0.05;
	}
	wait 2;
	level.callbackPlayerDamage = ::wrapDamage;
	player = players[0];
	away = player.origin;

	mines = getentarray("minefield", "targetname");
	for (i = 0; i < mines.size; i++)
	{
		mines[i] thread watch_trigger(mines[i] getEntityNumber());
		mines[i] thread watch_mine_touch(mines[i] getEntityNumber());
	}
	player thread watch_player_touch();

	at = (-10936.5, 12051.0, -29.7);
	if (getCvar("probe_mode") == "ammo")
	{
		ammo(player, at, away);
		return;
	}
	names = [];
	names[0] = "a";
	names[1] = "b";
	names[2] = "c";
	offsets = [];
	offsets[0] = (8, 0, 16);
	offsets[1] = (0, 8, 16);
	offsets[2] = (-8, 0, 16);
	items = [];
	for (i = 0; i < 3; i++)
	{
		items[i] = spawn("item_health", at + offsets[i]);
		items[i].probe_name = names[i];
		items[i] thread watch_touch(names[i]);
	}
	wait 1;
	phase(player, at, away, "spawned", items);

	// One unit along: Scr_SetOrigin links without an unlink.
	items[0].origin = items[0].origin + (1, 0, 0);
	wait 0.1;
	phase(player, at, away, "kept", items);

	// Far off and back: two links, the second landing at its node's head.
	back = items[0].origin;
	items[0].origin = back + (0, -4000, 0);
	wait 0.1;
	items[0].origin = back;
	wait 0.1;
	phase(player, at, away, "moved", items);

	logPrint("PROBE done\n");
}
