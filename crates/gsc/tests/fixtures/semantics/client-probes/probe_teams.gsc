//	`G_FindTeams`' chains as `Use_Item` walks them, for
//	docs/research/cod11-server-handshake.md ("Console commands over rcon")
//	and cod11-items.md. mp_carentan only, no client, with the entity blocks
//	of crates/server/tests/fixtures/teams/mp_carentan-teams.ent appended to
//	its entity lump (the fixture header has the recipe).
//
//	A second in, the script_origin m1b, a slave of m1a, takes a moveTo:
//	`G_RunMover` skips a slave, so it moves only inside its master's
//	`G_MoverTeam`, which runs once m1a takes a moveTo of its own a second
//	later. Both origins are logged each frame from the first moveTo for two
//	seconds, and each movedone as it comes.
//
//	Then a script_origin is the user. Each teamed item is used six
//	times 50 ms apart: w1b (a slave of the item w1a), w2d (a slave of the
//	script_origin w2m) and w3f (a slave of the func_tramcar w3t, its own
//	teammaster); then the unteamed item solo once; then w1a, the master of
//	w1, whose null teammaster is `G_Error`'s. The pick and the draw behind
//	it are not script-visible: tools/re/gdb_teams.py logs them.
//
//	PROBE mover <time> <m1a origin> <m1b origin>
//	PROBE movedone <name> <time>
//	PROBE user <num>
//	PROBE ent <targetname> <num>
//	PROBE use <targetname> <time>
//	PROBE master <time>
//	PROBE survived                 not reached on retail
//	Run by tools/run_probe.sh client-probes/probe_teams mp_carentan.

main()
{
	thread run();
	maps\mp\gametypes\dm::main();
}

run()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	movers();
	user = spawn("script_origin", (0, 0, 0));
	logPrint("PROBE user " + user getEntityNumber() + "\n");
	names = [];
	names[0] = "w1a";
	names[1] = "w1b";
	names[2] = "w1c";
	names[3] = "w2m";
	names[4] = "w2d";
	names[5] = "w2e";
	names[6] = "w3t";
	names[7] = "w3f";
	names[8] = "solo";
	for (i = 0; i < names.size; i++)
	{
		e = getent(names[i], "targetname");
		if (isdefined(e))
			logPrint("PROBE ent " + names[i] + " " + e getEntityNumber() + "\n");
		else
			logPrint("PROBE ent " + names[i] + " none\n");
	}
	use("w1b", user, 6);
	use("w2d", user, 6);
	use("w3f", user, 6);
	use("solo", user, 1);
	wait 0.5;
	logPrint("PROBE master " + getTime() + "\n");
	e = getent("w1a", "targetname");
	e useBy(user);
	logPrint("PROBE survived\n");
}

movers()
{
	a = getent("m1a", "targetname");
	b = getent("m1b", "targetname");
	a thread movedone("m1a");
	b thread movedone("m1b");
	b moveTo((0, 0, 200), 0.5);
	for (i = 0; i < 40; i++)
	{
		if (i == 20)
			a moveTo((0, 0, 50), 0.5);
		logPrint("PROBE mover " + getTime() + " " + a.origin + " " + b.origin + "\n");
		wait 0.05;
	}
	b delete();
	for (i = 0; i < 6; i++)
	{
		logPrint("PROBE deleted " + getTime() + " " + isdefined(b) + "\n");
		wait 0.05;
	}
}

movedone(name)
{
	for (;;)
	{
		self waittill("movedone");
		logPrint("PROBE movedone " + name + " " + getTime() + "\n");
	}
}

use(name, user, n)
{
	e = getent(name, "targetname");
	for (i = 0; i < n; i++)
	{
		logPrint("PROBE use " + name + " " + getTime() + "\n");
		e useBy(user);
		wait 0.05;
	}
}
