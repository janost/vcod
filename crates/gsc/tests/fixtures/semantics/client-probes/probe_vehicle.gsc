//	United Offensive vehicle capture (docs/research/coduo-diff.md, 9 and 18):
//	stock tdm with the map's vehicles, plus a placement. Client 0 is put 110
//	units off the first vehicle's right flank and client 1 off its left, each
//	facing the hull (`probe_side` units out and `probe_pitch` degrees under
//	10 down when set), once per spawn, so a `--probe-vehicle driver` and a
//	`--probe-vehicle gunner` client can press use into seats 1 and 2. Logs
//	the tanks once and every seat change. A third client is put 350 units
//	out along `probe_watch_yaw` to watch. UO only: `script_vehicle`,
//	`isInVehicle` and the `activated` notify do not exist in 1.1.
//
//	With `probe_respawn N`: N seconds after a seat takes a player, the
//	script spawns him again as a player 200 units off the vehicle's
//	starting point to its starting left and 48 up, facing 0, and logs what it reads
//	straight after the `spawn` and a frame later:
//
//	PROBE seat <time> <entity number> <origin> <angles>
//	PROBE seatnext <time> <entity number> <origin> <angles>
//
//	With `probe_suicide S`: S seconds after a seat first takes a player, the
//	script kills him with `suicide()` where he sits and logs, every frame
//	for 3 s, his session, `isInVehicle` and origin:
//
//	PROBE suicide <time> <entity number> <origin>
//	PROBE dead <time> <entity number> <sessionstate> <isInVehicle> <origin>
//
//	With `probe_bones` set, every frame each seated player's bones:
//
//	PROBE bone <time> <client> <seat> <tag> <origin>
//
//	With `probe_closespeed` or `probe_harc` set, every vehicle's field of
//	that name takes the value as the vehicles are logged
//	(coduo-vehicles.md 17).
//
//	Clients 0 and 1 log where the spawn left them as they are placed:
//
//	PROBE spawned <time> <entity number> <origin> <angles>

main()
{
	thread log_vehicles();
	thread watch_places();
	thread log_bones();
	maps\mp\gametypes\tdm::main();
}

log_vehicles()
{
	wait 1;
	vehs = getentarray("script_vehicle", "classname");
	for (i = 0; i < vehs.size; i++)
	{
		if (getcvar("probe_closespeed") != "")
			vehs[i].closespeed = getcvarfloat("probe_closespeed");
		if (getcvar("probe_harc") != "")
			vehs[i].harc = getcvarfloat("probe_harc");
		logPrint("PROBE vehicle " + vehs[i] getEntityNumber() + " " + vehs[i].vehicletype + " " + vehs[i].origin + " " + vehs[i].angles + "\n");
		vehs[i] thread log_seats();
		vehs[i] log_clearance();
	}
}

//	How far the ground is clear around a tank, every 45 degrees of world
//	yaw at hull height, so the client's script can drive into the open.
log_clearance()
{
	start = self.origin + (0, 0, 40);
	line = "PROBE clear " + self getEntityNumber();
	for (yaw = 0; yaw < 360; yaw += 45)
	{
		f = anglestoforward((0, yaw, 0));
		end = start + (f[0] * 1500, f[1] * 1500, 0);
		trace = bullettrace(start, end, false, self);
		line = line + " " + yaw + ":" + trace["fraction"] * 1500;
	}
	logPrint(line + "\n");
}

log_seats()
{
	for (;;)
	{
		self waittill("activated", vehpos, activator);
		activator.probe_seat = vehpos;
		if (getcvarint("probe_respawn") > 0)
			activator thread respawn(self, getcvarint("probe_respawn"));
		if (getcvarfloat("probe_suicide") > 0 && !isdefined(activator.probe_suicided))
		{
			activator.probe_suicided = 1;
			activator thread suicide_seated(getcvarfloat("probe_suicide"));
		}
		logPrint("PROBE activated " + gettime() + " " + self getEntityNumber() + " seat " + vehpos + " by " + activator getEntityNumber() + "\n");
	}
}

//	The vehicle the capture uses: the lowest-numbered `script_vehicle`, of
//	the `probe_vehicletype` cvar's type when that is set.
probe_vehicle()
{
	want = getcvar("probe_vehicletype");
	vehs = getentarray("script_vehicle", "classname");
	best = undefined;
	for (i = 0; i < vehs.size; i++)
	{
		if (want != "" && vehs[i].vehicletype != want)
			continue;
		if (!isdefined(best) || vehs[i] getEntityNumber() < best getEntityNumber())
			best = vehs[i];
	}
	return best;
}

watch_places()
{
	wait 2;
	veh = probe_vehicle();
	if (!isdefined(veh))
	{
		logPrint("PROBE place unsupported " + getcvar("mapname") + "\n");
		return;
	}
	level.probe_home = veh.origin;
	level.probe_home_angles = veh.angles;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place(veh);
		wait 0.05;
	}
}

//	Early returns, as probe_turret's try_place: sessionstate is undefined
//	before the first spawn.
try_place(veh)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
	{
		self.probe_placed = undefined;
		return;
	}
	if (isdefined(self.probe_placed))
		return;
	self.probe_placed = 1;
	if (self getEntityNumber() >= 2)
	{
		//	A watcher: 350 units out from where the vehicle started, along
		//	the `probe_watch_yaw` cvar's world yaw, facing back.
		f = anglestoforward((0, getcvarint("probe_watch_yaw"), 0));
		spot = level.probe_home + (f[0] * 350, f[1] * 350, 8);
		yaw = vectortoangles(level.probe_home - spot)[1];
		self setorigin(spot);
		self setplayerangles((0, yaw, 0));
		logPrint("PROBE place " + gettime() + " " + self getEntityNumber() + " " + spot + " " + yaw + "\n");
		return;
	}
	//	Where the spawn left him, before the placement moves him.
	logPrint("PROBE spawned " + gettime() + " " + self getEntityNumber() + " " + self.origin + " " + self.angles + "\n");
	side = getcvarint("probe_side");
	if (side == 0)
		side = 110;
	if (self getEntityNumber() != 0)
		side = 0 - side;
	right = anglestoright(veh.angles);
	spot = veh.origin + (right[0] * side, right[1] * side, 8);
	yaw = vectortoangles(veh.origin - spot)[1];
	self setorigin(spot);
	self setplayerangles((getcvarint("probe_pitch") + 10, yaw, 0));
	logPrint("PROBE place " + gettime() + " " + self getEntityNumber() + " " + spot + " " + yaw + "\n");
}

//	The driver's hands follow the steer's blend; the head and the chest
//	the controls. A script_origin linked to each bone (UO has no
//	`getTagOrigin`), logged every frame while client 0 sits in a seat;
//	a link pass after the client's end frame reads the pose that frame left.
log_bones()
{
	//	Read after the load: `run_probe.sh`'s `+set`s land after `+map`.
	wait 1;
	if (!getcvarint("probe_bones"))
		return;
	tags = [];
	tags[0] = "bip01 l hand";
	tags[1] = "bip01 r hand";
	tags[2] = "bip01 head";
	tags[3] = "bip01 spine2";
	tags[4] = "bip01 l foot";
	for (;;)
	{
		wait 0.05;
		players = getentarray("player", "classname");
		p = undefined;
		for (i = 0; i < players.size; i++)
			if (players[i] getEntityNumber() == 0)
				p = players[i];
		if (!isdefined(p) || !(p isInVehicle()) || !isdefined(p.probe_seat))
			continue;
		if (!isdefined(p.probe_marks))
		{
			p.probe_marks = [];
			for (j = 0; j < tags.size; j++)
			{
				p.probe_marks[j] = spawn("script_origin", p.origin);
				p.probe_marks[j] linkto(p, tags[j], (0, 0, 0), (0, 0, 0));
			}
		}
		for (j = 0; j < tags.size; j++)
			logPrint("PROBE bone " + gettime() + " 0 " + p.probe_seat + " " + tags[j] + " " + p.probe_marks[j].origin + "\n");
	}
}

respawn(veh, secs)
{
	self endon("disconnect");
	wait secs;
	right = anglestoright(level.probe_home_angles);
	spot = level.probe_home + (right[0] * -200, right[1] * -200, 48);
	self spawn(spot, (0, 0, 0));
	logPrint("PROBE seat " + getTime() + " " + self getEntityNumber() + " " + self.origin + " " + self.angles + "\n");
	wait 0.05;
	logPrint("PROBE seatnext " + getTime() + " " + self getEntityNumber() + " " + self.origin + " " + self.angles + "\n");
}

suicide_seated(secs)
{
	self endon("disconnect");
	wait secs;
	logPrint("PROBE suicide " + getTime() + " " + self getEntityNumber() + " " + self.origin + "\n");
	self suicide();
	for (i = 0; i < 60; i++)
	{
		logPrint("PROBE dead " + getTime() + " " + self getEntityNumber() + " " + self.sessionstate + " " + self isInVehicle() + " " + self.origin + "\n");
		wait 0.05;
	}
}
