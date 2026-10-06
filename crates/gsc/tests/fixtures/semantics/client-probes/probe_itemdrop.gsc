//	Item flight, landing and respawn capture's server half, on mp_carentan
//	with one allied player. Two respawns first, with the full loadout: an
//	item_health whose `wait` is 3 and a colt spawned with spawnflags 8, each
//	taken by walking onto it. Then four `dropItem` drops (onto flat ground,
//	into a wall, down a slope, and the non-weapon `item_health`), and three
//	script spawns (one in the air, one dropHealth-style at the feet, one
//	suspended with spawnflags 1). Every tracked item logs its `origin` and
//	`angles` on each frame they change. Run by tools/run_probe.sh with a
//	--probe-items --probe-team allies client.

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

stand(spot, yaw)
{
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
}

run()
{
	flat = (838, 2222, -23);
	wall = (836, -646, 6);
	slope = (468, -822, 32);
	away = (900, 1930, -42);

	wait 3;
	logPrint("PROBE start " + getTime() + "\n");

	//	Respawn by `random`: health under max, the item spawned at the feet.
	//	`wait` is a keyword, so a script cannot write that field.
	self stand(flat, 90);
	wait 1;
	self.health = 40;
	h = spawn("item_health", self.origin + (0, 0, 1));
	h.random = 0.5;
	logPrint("PROBE spawned randomhealth " + getTime() + " " + h getEntityNumber() + " " + h.origin + "\n");
	h thread track("randomhealth");
	h thread watch_trigger("randomhealth");
	wait 0.5;
	self stand(away, 0);
	self.health = 100;
	wait 4;

	//	Respawn by spawnflags 8: an owned colt with its reserve emptied.
	self stand(flat, 90);
	self setWeaponSlotAmmo("pistol", 0);
	wait 0.5;
	c = spawn("mpweapon_colt", self.origin + (0, 0, 1), 8);
	logPrint("PROBE spawned respawncolt " + getTime() + " " + c getEntityNumber() + " " + c.origin + "\n");
	c thread track("respawncolt");
	c thread watch_trigger("respawncolt");
	wait 1;
	self stand(away, 0);
	wait 7;

	self stand(flat, 90);
	wait 1;
	self drop("flat", self getcurrentweapon());
	wait 3;

	self stand(wall, 0);
	wait 1;
	self drop("wall", "colt_mp");
	wait 3;

	self stand(slope, 270);
	wait 1;
	self drop("slope", "fraggrenade_mp");
	wait 3;

	self stand(flat, 270);
	wait 1;
	self drop("health", "item_health");
	wait 3;

	self stand(away, 0);
	wait 1;
	s = spawn("mpweapon_m1carbine", flat + (0, 60, 72));
	s.angles = (0, 45, 0);
	logPrint("PROBE spawned air " + getTime() + " " + s getEntityNumber() + " " + s.origin + " " + s.angles + "\n");
	s thread track("air");
	wait 0.5;
	d = spawn("item_health", slope + (0, 0, 1));
	d.angles = (0, 123, 0);
	logPrint("PROBE spawned feet " + getTime() + " " + d getEntityNumber() + " " + d.origin + " " + d.angles + "\n");
	d thread track("feet");
	wait 0.5;
	u = spawn("mpweapon_colt", flat + (60, 0, 40), 1);
	logPrint("PROBE spawned hang " + getTime() + " " + u getEntityNumber() + " " + u.origin + " " + u.angles + "\n");
	u thread track("hang");
	wait 4;
	logPrint("PROBE done " + getTime() + "\n");
}

drop(tag, name)
{
	item = self dropItem(name);
	if (!isdefined(item))
	{
		logPrint("PROBE drop " + tag + " " + getTime() + " undefined\n");
		return;
	}
	logPrint("PROBE drop " + tag + " " + getTime() + " " + item getEntityNumber() + " " + item.classname + " player " + self.origin + " " + self.angles + " item " + item.origin + " " + item.angles + "\n");
	item thread track(tag);
}

//	One line per frame the origin or angles changed, until 3 s pass with
//	neither moving.
track(tag)
{
	self endon("death");
	o = self.origin;
	a = self.angles;
	still = 0;
	while (still < 60)
	{
		wait 0.05;
		if (self.origin == o && self.angles == a)
		{
			still++;
			continue;
		}
		still = 0;
		o = self.origin;
		a = self.angles;
		logPrint("PROBE at " + tag + " " + getTime() + " " + o + " " + a + "\n");
	}
	logPrint("PROBE rest " + tag + " " + getTime() + " " + o + " " + a + "\n");
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
