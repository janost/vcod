//	Which hudelems a retail client draws at intermission (pm_type 5,
//	docs/research/cod11-hud-protocol.md 9.1), the server half of
//	tools/capture/retail-client step `intermission`. Stock dm, any patch.
//
//	Every client that reaches the spectator state is put on allies with
//	the m1 carbine and spawned through dm's own spawnPlayer, so the retail
//	client clicks no menu. Two solid hudelems stand outside the
//	scoreboard's frame (120..520 across): a level element, magenta, at
//	(16, 216) and a client element, green, at (576, 216), both 48 by 48 in
//	the 640x480 virtual screen, aligned left and top. The capture's
//	screenshots are sampled there (tools/capture/retail-client/analyze.py).
//
//	`rc_phase` (set from the server console) moves every player:
//	`play` (the default) leaves him alive, `intermission` runs dm's
//	spawnIntermission on him, which puts him in pm_type 5 with the
//	scoreboard up. Each player's `rc_shot` cvar names the phase's three
//	screenshots (`rc_<phase>_<n>`). Each change is logged:
//
//	PROBE hud <time> level 16 216 48 48 1 0 1
//	PROBE hud <time> client <entity number> 576 216 48 48 0 1 0
//	PROBE phase <time> <phase>

main()
{
	precacheShader("white");
	thread autojoin("m1carbine_mp");
	thread phases();
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
			p thread joined();
		}
		wait 0.05;
	}
}

joined()
{
	if (!isdefined(level.rc_box))
	{
		level.rc_box = box(newHudElem(), 16, (1, 0, 1));
		logPrint("PROBE hud " + getTime() + " level 16 216 48 48 1 0 1\n");
	}
	self.rc_box = box(newClientHudElem(self), 576, (0, 1, 0));
	self shots(getcvar("rc_phase"));
	logPrint("PROBE hud " + getTime() + " client " + self getEntityNumber() + " 576 216 48 48 0 1 0\n");
}

box(e, x, color)
{
	e.x = x;
	e.y = 216;
	e.alignX = "left";
	e.alignY = "top";
	e.color = color;
	e.alpha = 1;
	e setShader("white", 48, 48);
	return e;
}

phases()
{
	setcvar("rc_phase", "play");
	phase = "play";
	logPrint("PROBE phase " + getTime() + " " + phase + "\n");
	for (;;)
	{
		wait 0.05;
		next = getcvar("rc_phase");
		if (next == phase)
			continue;
		phase = next;
		logPrint("PROBE phase " + getTime() + " " + phase + "\n");
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			players[i] shots(phase);
			if (phase == "intermission")
				players[i] thread maps\mp\gametypes\dm::spawnIntermission();
		}
	}
}

//	The client's `rc_shot`: three named screenshots of `phase`, which the
//	capture's console line `vstr rc_shot` runs.
shots(phase)
{
	s = "screenshot rc_" + phase + "_1; wait 25; screenshot rc_" + phase + "_2; wait 25; screenshot rc_" + phase + "_3";
	self setClientCvar("rc_shot", s);
}
