//	The respawn draw's sign, on mp_carentan with one allied player: a colt
//	spawned with spawnflags 8 and `random` 4, run with
//	`+set g_weaponrespawn 20`, taken six times by walking onto it with the
//	pistol's reserve emptied. Each return comes `20 + (int)(4 * crandom())`
//	seconds after its take, floored at 1: 9 to 16 with 1.1d's (-3, -1]
//	reading, 16 to 23 with 1.5b's and UO's [-1, 1). The player waits 24 s
//	away between takes, past the longest. Logs `PROBE trigger` per take;
//	the --probe-items --probe-team allies client's ITEM lines carry the
//	returns.

main()
{
	thread watch_players();
	maps\mp\gametypes\dm::main();
}

watch_players()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_start();
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_start()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_running))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_running = 1;
	self thread run();
}

stand(spot)
{
	self setorigin(spot);
	self setplayerangles((0, 0, 0));
}

run()
{
	flat = (838, 2222, -23);
	away = (900, 1930, -42);

	wait 3;
	logPrint("PROBE start " + getTime() + " " + getcvar("g_weaponrespawn") + "\n");
	self stand(away);
	wait 0.5;
	c = spawn("mpweapon_colt", flat + (0, 0, 1), 8);
	c.random = 4;
	logPrint("PROBE spawned colt " + getTime() + " " + c getEntityNumber() + "\n");
	c thread watch_trigger("colt");
	wait 1;
	for (i = 0; i < 6; i++)
	{
		self setWeaponSlotAmmo("pistol", 0);
		self stand(flat);
		wait 1;
		self stand(away);
		wait 24;
	}
	logPrint("PROBE done " + getTime() + "\n");
}

watch_trigger(tag)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", player);
		logPrint("PROBE trigger " + tag + " " + getTime() + " " + player getEntityNumber() + "\n");
	}
}
