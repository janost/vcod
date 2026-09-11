//	The lookat trigger's fires and isLookingAt's answer, one logPrint each,
//	for the A/B in crates/server/tests/sd_plant_ab.rs. Run by
//	tools/run_probe.sh with the --probe-plant / --probe-defuse pair on the
//	server; the client halves write their own fixtures.

main()
{
	thread watch_lookats();
	maps\mp\gametypes\sd::main();
}

watch_lookats()
{
	wait 1;
	ents = getentarray("trigger_lookat", "classname");
	for (i = 0; i < ents.size; i++)
	{
		num = ents[i] getEntityNumber();
		logPrint("PROBE watch " + num + " trigger_lookat\n");
		ents[i] thread watch_one(num);
		ents[i] thread poll_lookingat(num);
	}
	logPrint("PROBE lookats " + ents.size + "\n");
}

//	One line per "trigger" notify: the trigger, the toucher, where it stood,
//	and the server clock, which is what pairs a fire with the probe's own
//	!trace line.
watch_one(num)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", other);
		logPrint("PROBE fire " + num + " " + getTime() + " " + other getEntityNumber() + " " + other.origin + "\n");
	}
}

//	Once a server frame, every player isLookingAt answers true for.
poll_lookingat(num)
{
	self endon("death");
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (players[i] islookingat(self))
				logPrint("PROBE looking " + num + " " + getTime() + " " + players[i] getEntityNumber() + "\n");
		}
		wait 0.05;
	}
}
