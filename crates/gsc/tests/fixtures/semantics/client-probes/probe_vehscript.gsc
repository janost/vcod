//	United Offensive: the vehicle script surface no stock MP script uses
//	(docs/research/coduo-vehicles.md 17): the `closespeed`, `harc` and
//	`speed` fields, `getWheelSurface`, the path methods and `freeVehicle`.
//	Stock tdm with the map's vehicles; no client needed. The `probe_case`
//	cvar picks one case per run, since a builtin's error ends the server
//	at `developer 1`:
//
//	read      every vehicle by number: type, the three fields, every
//	          wheel's surface; then the fields written and read back, the
//	          path speed setters, and the speed a frame on
//	free      `freeVehicle` on the lowest-numbered vehicle: its `death`
//	          notify, its threads, what the handle and `getEntArray` read
//	<method>  one erroring call, named by the case (see `fatal`)
//
//	Logs, each as `PROBE <step> ...`:
//
//	veh       number, vehicletype, closespeed, harc, speed, getSpeedMPH
//	wheel     number, wheel name, getWheelSurface
//	set       number, closespeed, harc read back after the write
//	speed     number, speed and getSpeedMPH after the setters and a frame
//	at        the call about to run
//	after     the call returned
//
//	COD_VERSION=uo PORT=<p> PROBE_SECS=12 \
//	    tools/run_probe.sh client-probes/probe_vehscript mp_kharkov \
//	    +set probe_case <case>

main()
{
	thread watch();
	maps\mp\gametypes\tdm::main();
}

sorted()
{
	vehs = getentarray("script_vehicle", "classname");
	for (i = 0; i < vehs.size; i++)
		for (j = i + 1; j < vehs.size; j++)
			if (vehs[j] getEntityNumber() < vehs[i] getEntityNumber())
			{
				t = vehs[i];
				vehs[i] = vehs[j];
				vehs[j] = t;
			}
	return vehs;
}

first(type)
{
	vehs = sorted();
	for (i = 0; i < vehs.size; i++)
		if (type == "" || vehs[i].vehicletype == type)
			return vehs[i];
	return undefined;
}

wheels(veh)
{
	names[0] = "front_left";
	names[1] = "front_right";
	names[2] = "back_left";
	names[3] = "back_right";
	names[4] = "middle_left";
	names[5] = "middle_right";
	if (veh.vehicletype == "flak88_mp")
		return;
	n = 4;
	if (veh.vehicletype != "willyjeep_mp" && veh.vehicletype != "horch_mp"
		&& veh.vehicletype != "gaz67b_mp")
		n = 6;
	for (w = 0; w < n; w++)
		logPrint("PROBE wheel " + veh getEntityNumber() + " " + names[w] + " "
			+ veh getWheelSurface(names[w]) + "\n");
}

describe(veh)
{
	logPrint("PROBE veh " + veh getEntityNumber() + " " + veh.vehicletype + " "
		+ veh.closespeed + " " + veh.harc + " " + veh.speed + " " + veh getSpeedMPH() + "\n");
}

read()
{
	vehs = sorted();
	for (i = 0; i < vehs.size; i++)
	{
		describe(vehs[i]);
		wheels(vehs[i]);
	}
	veh = vehs[0];
	veh.closespeed = 0.5;
	veh.harc = 10;
	logPrint("PROBE set " + veh getEntityNumber() + " " + veh.closespeed + " " + veh.harc + "\n");
	veh setWaitSpeed(10);
	veh setSpeed(5, 2);
	veh resumeSpeed(3);
	logPrint("PROBE after setters\n");
	wait 0.05;
	logPrint("PROBE speed " + veh getEntityNumber() + " " + veh.speed + " " + veh getSpeedMPH() + "\n");
	veh.speed = 176;
	logPrint("PROBE speed " + veh getEntityNumber() + " " + veh.speed + " " + veh getSpeedMPH() + "\n");
	wait 0.05;
	logPrint("PROBE speed " + veh getEntityNumber() + " " + veh.speed + " " + veh getSpeedMPH() + "\n");
}

deathwatch(tag)
{
	self waittill("death");
	logPrint("PROBE death " + tag + " " + gettime() + "\n");
	wait 0.05;
	logPrint("PROBE death_after " + tag + " " + gettime() + "\n");
}

ender()
{
	self endon("death");
	wait 5;
	logPrint("PROBE ender_ran " + gettime() + "\n");
}

selfthread()
{
	wait 0.05;
	logPrint("PROBE self_ran " + gettime() + "\n");
}

free()
{
	veh = first("");
	num = veh getEntityNumber();
	logPrint("PROBE free " + num + " " + veh.vehicletype + " " + veh.health + " " + veh.origin + " "
		+ getentarray("script_vehicle", "classname").size + "\n");
	veh thread deathwatch("self");
	level thread watchfrom(veh);
	veh thread ender();
	veh thread selfthread();
	veh.probe_field = 7;
	veh freeVehicle();
	logPrint("PROBE after " + gettime() + " " + isdefined(veh) + "\n");
	wait 0.05;
	logPrint("PROBE after " + gettime() + " " + isdefined(veh) + "\n");
	corpses = getentarray("script_vehicle_corpse", "classname");
	logPrint("PROBE corpses " + corpses.size + "\n");
	for (i = 0; i < corpses.size; i++)
	{
		c = corpses[i];
		logPrint("PROBE corpse " + c getEntityNumber() + " " + c.classname + " " + isdefined(c.probe_field)
			+ " " + isdefined(c.vehicletype) + " " + c.speed + " " + c.health + " " + c.origin + "\n");
	}
	logPrint("PROBE vehicles " + getentarray("script_vehicle", "classname").size + "\n");
	wait 1;
	logPrint("PROBE done " + gettime() + "\n");
}

watchfrom(veh)
{
	veh waittill("death");
	logPrint("PROBE death level " + gettime() + "\n");
}

fatal(c)
{
	jeep = first("horch_mp");
	tank = first("panzeriv_mp");
	logPrint("PROBE at " + c + "\n");
	if (c == "attachpath_ent")
		tank attachPath(jeep);
	else if (c == "attachpath_int")
		tank attachPath(5);
	else if (c == "startpath")
		tank startPath();
	else if (c == "setswitchnode")
		tank setSwitchNode(jeep, jeep);
	else if (c == "setwaitnode")
		tank setWaitNode(jeep);
	else if (c == "setspeed_neg")
		tank setSpeed(-1, 1);
	else if (c == "setspeed_accel")
		tank setSpeed(1, -1);
	else if (c == "setspeed_one")
		tank setSpeed(5);
	else if (c == "setwaitspeed_neg")
		tank setWaitSpeed(-1);
	else if (c == "resumespeed_neg")
		tank resumeSpeed(-1);
	else if (c == "wheel_bad")
		tank getWheelSurface("front");
	else if (c == "wheel_middle")
		jeep getWheelSurface("middle_left");
	else if (c == "wheel_artillery")
		first("flak88_mp") getWheelSurface("front_left");
	else if (c == "wheel_int")
		tank getWheelSurface(3);
	else if (c == "freevehicle_twice")
	{
		tank freeVehicle();
		logPrint("PROBE after first\n");
		tank freeVehicle();
	}
	logPrint("PROBE after " + c + "\n");
}

watch()
{
	wait 1;
	c = getcvar("probe_case");
	if (c == "read" || c == "")
		read();
	else if (c == "free")
		free();
	else
		fatal(c);
}
