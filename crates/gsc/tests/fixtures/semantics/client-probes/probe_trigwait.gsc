//	Touch_Multi's wait arm, on mp_carentan with its entity lump patched so
//	the four trigger_multiples carry `wait` keys (`wait` is a keyword, so a
//	script cannot write the field): bombzone_A wait 5, bombzone_B wait 0,
//	auto1 wait -1, and auto2 turned into a trigger_once. The patch keeps every
//	byte offset; crates/server/tests/linkto_ab.rs applies the same one.
//	Once an allied player has spawned it parks the player and moves one
//	trigger at a time onto the player by an origin write:
//
//		PROBE f <targetname> <num> <classname>
//		PROBE d <phase> <time> <defined> <trigger_multiple + trigger_once count>
//		PROBE n <tag> <time> <toucher>
//
//	w5: bombzone_A. w0: bombzone_B. wlink: auto1 after enableLinkTo, no
//	parent. once: auto2.
//
//	Ends on a deliberate fatal (enableLinkTo on a script_origin), so the last
//	PROBE_FATAL line is part of the measurement.
//	Run by tools/run_probe.sh with a --probe-team allies client.

main()
{
	trigs = triggers();
	for (i = 0; i < trigs.size; i++)
		trigs[i].script_gameobjectname = "dm";
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
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_run();
		wait 0.05;
	}
}

try_run()
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
	self thread phases();
}

triggers()
{
	a = getentarray("trigger_multiple", "classname");
	b = getentarray("trigger_once", "classname");
	for (i = 0; i < b.size; i++)
		a[a.size] = b[i];
	return a;
}

phases()
{
	wait 3;
	self setorigin((-512, 2688, -16));
	self setplayerangles((0, 90, 0));
	wait 1;
	//	Models *3, *4, *7 and *10 all reach from below -44 to above 8 about
	//	their origins, so an origin 16 above the feet puts the player inside.
	at = self.origin + (0, 0, 16);

	names = [];
	names[0] = "bombzone_A";
	names[1] = "bombzone_B";
	names[2] = "auto1";
	names[3] = "auto2";
	trig = [];
	for (i = 0; i < names.size; i++)
	{
		ts = getentarray(names[i], "targetname");
		for (k = 0; k < ts.size; k++)
			if (ts[k].classname == "trigger_multiple" || ts[k].classname == "trigger_once")
				trig[i] = ts[k];
		t = trig[i];
		logPrint("PROBE f " + names[i] + " " + t getEntityNumber() + " " + t.classname + "\n");
	}

	run(trig[0], "w5", at);
	run(trig[1], "w0", at);
	trig[2] enableLinkTo();
	run(trig[2], "wlink", at);
	run(trig[3], "once", at);

	s = spawn("script_origin", self.origin);
	logPrint("PROBE at end " + getTime() + "\n");
	s enableLinkTo();
	logPrint("PROBE end_survived\n");
}

run(t, phase, at)
{
	home = t.origin;
	t thread notes(phase);
	logPrint("PROBE at " + phase + " " + getTime() + "\n");
	t.origin = at;
	for (i = 0; i < 20; i++)
	{
		logPrint("PROBE d " + phase + " " + getTime() + " " + isdefined(t) + " " + triggers().size + "\n");
		wait 0.05;
	}
	if (isdefined(t))
		t.origin = home;
	wait 0.5;
}

notes(tag)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", who);
		logPrint("PROBE n " + tag + " " + getTime() + " " + who getEntityNumber() + "\n");
	}
}
