//	A retail client's view kick in its usercmds around a hip shot
//	(docs/research/cod11-combat.md 15.7, "The frame a shot kicks"), the
//	server half of tools/capture/retail-client step `kick`. Stock dm, any
//	patch, under vcod-server's --client-log, which records the cmds.
//
//	Every client that reaches the spectator state is put on allies with
//	the m1 carbine (hip view kick pitch 40/40, cod11-combat.md 15.7) and
//	spawned through dm's own spawnPlayer, so the retail client clicks no
//	menu; its view is levelled once:
//
//	PROBE ready <time> <entity number> <origin> <view yaw>
//
//	`rc_drive 1` (vcod's --probe-uo-weapon direct client standing in for
//	the retail one) also taps its trigger 12 times, 1.5 s apart, through
//	`PROBE_IN` lines, logged as
//
//	PROBE tap <time> <n>

main()
{
	thread autojoin("m1carbine_mp");
	maps\mp\gametypes\dm::main();
}

//	dm's connect leaves a client spectating with its team menu open; this
//	answers for it.
autojoin(weapon)
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (!isdefined(p.sessionstate) || p.sessionstate != "spectator")
				continue;
			if (isdefined(p.rc_joined))
				continue;
			p.rc_joined = true;
			p.pers["team"] = "allies";
			p.pers["weapon"] = weapon;
			p closeMenu();
			p maps\mp\gametypes\dm::spawnPlayer();
			p thread joined(weapon);
		}
		wait 0.05;
	}
}

joined(weapon)
{
	self giveMaxAmmo(weapon);
	self setPlayerAngles((0, self.angles[1], 0));
	logPrint("PROBE ready " + getTime() + " " + self getEntityNumber() + " " + self.origin + " " + self.angles[1] + "\n");
	if (getcvar("rc_drive") == "1")
		self thread drive();
}

drive()
{
	self endon("disconnect");
	wait 3;
	for (n = 0; n < 12; n++)
	{
		self iprintln("PROBE_IN 1 0 0");
		logPrint("PROBE tap " + getTime() + " " + n + "\n");
		wait 0.1;
		self iprintln("PROBE_IN 0 0 0");
		wait 1.4;
	}
}
