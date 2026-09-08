//	Every `"trigger"` notify the engine's touch pass raises, one logPrint per
//	fire, for the A/B in crates/server/tests/triggers_ab.rs.
//	Run by tools/run_probe.sh with a client walking the map; the client half is
//	`--net-probe --probe-triggers` and writes nothing of its own.
//	Groups are separate files because a script runtime error takes the whole
//	retail server down, so one fatal expression would cost every measurement
//	after it.

main()
{
	thread watch_triggers();

	//	The real gametype, so a client can answer the team menu and spawn.
	//	Without a spawned player nothing ever touches anything.
	maps\mp\gametypes\dm::main();
}

watch_triggers()
{
	//	The entity lump is loaded before this runs, but the map's own main()
	//	is not; a wait puts the scan after the whole bootstrap so a trigger
	//	the map script deletes is not watched and one it moves is watched
	//	where it ended up.
	wait 1;

	n = 0;
	n = n + watch_class("trigger_multiple");
	n = n + watch_class("trigger_once");
	n = n + watch_class("trigger_use");
	n = n + watch_class("trigger_lookat");
	n = n + watch_class("trigger_hurt");
	n = n + watch_class("trigger_damage");
	logPrint("PROBE triggers " + n + "\n");
}

//	Starts one watcher per entity of `classname` and logs the class census,
//	which is what says a fixture with no fire of some class had none to fire.
watch_class(classname)
{
	ents = getentarray(classname, "classname");
	for (i = 0; i < ents.size; i++)
	{
		num = ents[i] getEntityNumber();
		logPrint("PROBE watch " + num + " " + classname + "\n");
		ents[i] thread watch_one(num, classname);
	}
	return ents.size;
}

watch_one(num, classname)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", other);
		logPrint("PROBE fire " + num + " " + classname + " " + other getEntityNumber() + " " + other.origin + "\n");
	}
}
