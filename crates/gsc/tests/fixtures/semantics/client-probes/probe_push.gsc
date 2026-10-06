//	Mover push capture over items, mp_carentan only. Keeps
//	the bombzone_A brush model *5 as probe_ride does, drops three items on and
//	beside its floor slab (brush 4281, top z -22), and runs one verb per phase
//	on the brush model. Every server frame of a phase logs the three items'
//	origins and the mover's getorigin():
//
//		PROBE f <phase> <time> <a origin> <b origin> <c origin> <mover origin>
//
//	a: a carbine dropped onto the slab, the rider.
//	b: a health pack on the ground south of the slab, in the movey(-48) path.
//	c: a carbine hung in the air (spawnflags 1) in the same path, above b.
//
//	Run by tools/run_probe.sh client-probes/probe_push mp_carentan with a
//	--probe-items --probe-team allies client, the wire half: every item's
//	ground and trajectories as sent. The phases wait for that client, and
//	put it on the attackers' spawn, clear of the slab and in sight of it.

main()
{
	bms = getentarray("script_brushmodel", "classname");
	for (i = 0; i < bms.size; i++)
		bms[i].script_gameobjectname = "dm";
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
	bms = getentarray("script_brushmodel", "classname");
	bz = undefined;
	for (i = 0; i < bms.size; i++)
	{
		logPrint("PROBE bm " + bms[i] getEntityNumber() + " model '" + bms[i].model + "'\n");
		if (!isdefined(bz) || bms[i] getEntityNumber() < bz getEntityNumber())
			bz = bms[i];
	}
	if (!isdefined(bz))
	{
		logPrint("PROBE no_bz\n");
		return;
	}

	p = undefined;
	for (i = 0; i < 600 && !isdefined(p); i++)
	{
		wait 0.05;
		players = getentarray("player", "classname");
		for (j = 0; j < players.size; j++)
			if (players[j] playing())
				p = players[j];
	}
	if (isdefined(p))
	{
		p setorigin((-512, 2688, -16));
		wait 1;
	}

	a = spawn("mpweapon_m1carbine", (-215, 2463, -10));
	b = spawn("item_health", (-215, 2440, -20));
	c = spawn("mpweapon_m1carbine", (-200, 2440, -24), 1);
	wait 2;
	logPrint("PROBE items " + a getEntityNumber() + " " + b getEntityNumber() + " " + c getEntityNumber() + "\n");
	sample("rest", a, b, c, bz, 0.5);

	logPrint("PROBE at ride_up " + getTime() + "\n");
	bz movez(48, 2);
	sample("ride_up", a, b, c, bz, 3);

	logPrint("PROBE at ride_down " + getTime() + "\n");
	bz movez(-48, 2);
	sample("ride_down", a, b, c, bz, 3);

	logPrint("PROBE at ride_x " + getTime() + "\n");
	bz movex(48, 2);
	sample("ride_x", a, b, c, bz, 3);

	logPrint("PROBE at ride_x_back " + getTime() + "\n");
	bz movex(-48, 2);
	sample("ride_x_back", a, b, c, bz, 3);

	logPrint("PROBE at ride_yaw " + getTime() + "\n");
	bz rotateyaw(2, 1);
	sample("ride_yaw", a, b, c, bz, 2);

	logPrint("PROBE at ride_yaw_back " + getTime() + "\n");
	bz rotateyaw(-2, 1);
	sample("ride_yaw_back", a, b, c, bz, 2);

	logPrint("PROBE at push_y " + getTime() + "\n");
	bz movey(-48, 2);
	sample("push_y", a, b, c, bz, 3);

	logPrint("PROBE at push_y_back " + getTime() + "\n");
	bz movey(48, 2);
	sample("push_y_back", a, b, c, bz, 3);

	//	Lifted, carried over b where the push left it, and lowered onto it.
	logPrint("PROBE at lift " + getTime() + "\n");
	bz movez(80, 0.5);
	sample("lift", a, b, c, bz, 1);
	logPrint("PROBE at over " + getTime() + "\n");
	bz movey(-64, 0.5);
	sample("over", a, b, c, bz, 1);
	logPrint("PROBE at crush " + getTime() + "\n");
	bz movez(-80, 2);
	sample("crush", a, b, c, bz, 3);
	logPrint("PROBE done " + getTime() + "\n");
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
playing()
{
	if (!isdefined(self.sessionstate))
		return false;
	return self.sessionstate == "playing";
}

sample(phase, a, b, c, ent, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE f " + phase + " " + getTime() + " " + a.origin + " " + b.origin + " " + c.origin + " " + ent getorigin() + "\n");
		wait 0.05;
	}
}
