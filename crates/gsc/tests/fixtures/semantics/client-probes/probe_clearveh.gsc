//	United Offensive: `clearVehiclePosition` on a dropped weapon in a
//	vehicle's box (docs/research/coduo-vehicles.md 16). Stock tdm, no
//	client: past the reconnect limit the script adds a test client, stands
//	him over the lowest-numbered `script_vehicle` and has him drop an
//	`mp40_mp` there, then 512 units off it a second, sets waiters on both,
//	and a second on calls `clearVehiclePosition` on the vehicle. Then it
//	spawns script origins to read when the item's number is handed back.
//	UO's script `spawn` has no item classnames. Logged:
//
//	PROBE vehicle <number> <vehicletype> <origin>
//	PROBE item <tag> <number> <origin>
//	PROBE call <time>
//	PROBE death <tag> <time> <self> <argument number>
//	PROBE ended <tag> <time>
//	PROBE after <time> <isdefined near> <isdefined far>
//	PROBE spawn <time> <number>
//	PROBE done <time>
//	UO only.

main()
{
	precacheItem("mp40_mp");
	thread watch();
	maps\mp\gametypes\tdm::main();
}

entnum(e)
{
	if (!isdefined(e))
		return -1;
	return e getEntityNumber();
}

waiter(tag)
{
	self waittill("death", e);
	logPrint("PROBE death " + tag + " " + gettime() + " " + entnum(self) + " " + entnum(e) + "\n");
}

ender(tag)
{
	self endon("death");
	for (;;)
		wait 10;
}

ended(ent, tag)
{
	ent thread ender(tag);
	ent waittill("death");
	wait 0;
	logPrint("PROBE ended " + tag + " " + gettime() + "\n");
}

watch()
{
	wait 1;
	vehs = getentarray("script_vehicle", "classname");
	veh = undefined;
	for (i = 0; i < vehs.size; i++)
		if (!isdefined(veh) || vehs[i] getEntityNumber() < veh getEntityNumber())
			veh = vehs[i];
	if (!isdefined(veh))
	{
		logPrint("PROBE novehicle\n");
		return;
	}
	logPrint("PROBE vehicle " + veh getEntityNumber() + " " + veh.vehicletype + " " + veh.origin + "\n");
	wait 5;
	bot = addtestclient();
	if (!isdefined(bot))
	{
		logPrint("PROBE nobot\n");
		return;
	}
	wait 0.5;
	bot.sessionteam = "allies";
	bot.sessionstate = "playing";
	bot spawn(veh.origin + (0, 0, 120), (0, 0, 0));
	bot giveWeapon("mp40_mp");
	near = bot dropItem("mp40_mp");
	bot setOrigin(veh.origin + (512, 0, 120));
	bot giveWeapon("mp40_mp");
	far = bot dropItem("mp40_mp");
	bot setOrigin(veh.origin + (0, 1024, 120));
	if (!isdefined(near) || !isdefined(far))
	{
		logPrint("PROBE nodrop\n");
		return;
	}
	near thread waiter("near");
	far thread waiter("far");
	level thread ended(near, "near");
	wait 1;
	logPrint("PROBE item near " + entnum(near) + " " + near.origin + "\n");
	logPrint("PROBE item far " + entnum(far) + " " + far.origin + "\n");
	logPrint("PROBE call " + gettime() + "\n");
	veh clearVehiclePosition();
	logPrint("PROBE after " + gettime() + " " + isdefined(near) + " " + isdefined(far) + "\n");
	wait 0.05;
	logPrint("PROBE after " + gettime() + " " + isdefined(near) + " " + isdefined(far) + "\n");
	for (i = 0; i < 4; i++)
	{
		e = spawn("script_origin", (0, 0, 0));
		logPrint("PROBE spawn " + gettime() + " " + e getEntityNumber() + "\n");
		wait 0.05;
	}
	logPrint("PROBE done " + gettime() + "\n");
}
