//	Mounted MG's server half: places a gunner behind mp_carentan's turret at
//	(1712 1830 8) and a target in front of it, logs every turret once. The
//	hits themselves are the engine's D;/K; lines. Run by tools/run_probe.sh
//	with a --save-turret client (allies) and a --probe-team axis client.

main()
{
	//	Only a replay sets it before the load (run_probe.sh's +sets land
	//	after +map); a model attached past the bootstrap must be precached.
	if (getcvar("probe_head") != "")
		precacheModel(getcvar("probe_head"));
	thread log_turrets();
	thread watch_teleports();
	thread watch_delete();
	thread watch_pose();
	thread watch_respawn();
	thread watch_hitlog();
	thread watch_spawns();
	thread watch_gungrid();
	maps\mp\gametypes\dm::main();
}

//	With `probe_delete_after N`: the capture's gun is deleted N seconds in.
//	Only the A/B rig sets it (the release-on-delete test); retail runs leave
//	it unset.
watch_delete()
{
	secs = getcvarint("probe_delete_after");
	if (secs <= 0)
		return;
	wait secs;
	gun = probe_gun();
	if (isdefined(gun))
		gun delete();
}

log_turrets()
{
	wait 1;
	guns = getentarray("misc_mg42", "classname");
	for (i = 0; i < guns.size; i++)
		logPrint("PROBE turret " + guns[i] getEntityNumber() + " " + guns[i].origin + " " + guns[i].angles + "\n");
}

//	The gun the capture uses: the one nearest (1712 1830 8).
probe_gun()
{
	guns = getentarray("misc_mg42", "classname");
	best = undefined;
	for (i = 0; i < guns.size; i++)
	{
		if (!isdefined(best) || distance(guns[i].origin, (1712, 1830, 8)) < distance(best.origin, (1712, 1830, 8)))
			best = guns[i];
	}
	return best;
}

//	With `probe_teleport 1`: every allied player is put 40 units behind the
//	gun facing along it, every axis player 300 units in front facing back,
//	once per spawn, the first frame its sessionstate reads "playing":
//
//	PROBE place <entity number> <team> <spot> <yaw> <time>
watch_teleports()
{
	if (getcvar("probe_teleport") != "1")
		return;
	wait 1;
	gun = probe_gun();
	if (!isdefined(gun))
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place(gun);
		wait 0.05;
	}
}

//	Early returns rather than one compound test, the way probe_pickup's
//	try_teleport guards sessionstate: it is undefined before the first spawn.
try_place(gun)
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
	//	The spawn point the stock spawnPlayer picked, which a replay pins.
	if (getcvar("probe_spawnlog") == "1")
		logPrint("PROBE spawnat " + self getEntityNumber() + " " + self.origin + " " + self.angles + "\n");
	fwd = anglestoforward(gun.angles);
	if (self.pers["team"] == "allies")
	{
		spot = gun.origin - (fwd[0] * 40, fwd[1] * 40, 0);
		yaw = gun.angles[1];
	}
	else
	{
		spot = gun.origin + (fwd[0] * 300, fwd[1] * 300, 0);
		yaw = gun.angles[1] + 180;
	}
	//	With `probe_ground 1` the axis player lands on the ground under the
	//	spot instead of falling onto it.
	if (self.pers["team"] == "axis" && getcvar("probe_ground") == "1")
		spot = bullettrace(spot, spot - (0, 0, 200), false, self)["position"] + (0, 0, 0.125);
	self.probe_spot = spot;
	if (self.pers["team"] == "axis")
		self swap_head();
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
	logPrint("PROBE place " + self getEntityNumber() + " " + self.pers["team"] + " " + spot + " " + yaw + " " + getTime() + "\n");
}

//	With `probe_spawnlog 1`: the level time of every player's `spawned`
//	notify, which the stock spawnPlayer raises ahead of its `spawn`:
//
//	PROBE spawned <entity number> <time>
//
//	and, at each placement, where the spawn put him:
//
//	PROBE spawnat <entity number> <origin> <angles>
watch_spawns()
{
	if (getcvar("probe_spawnlog") != "1")
		return;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (isdefined(players[i].probe_spawnlog))
				continue;
			players[i].probe_spawnlog = 1;
			players[i] thread log_spawns();
		}
		wait 0.05;
	}
}

log_spawns()
{
	self endon("disconnect");
	for (;;)
	{
		self waittill("spawned");
		logPrint("PROBE spawned " + self getEntityNumber() + " " + getTime() + "\n");
	}
}

//	The head is a random draw from the team's alias (`xmodelalias/head_*`),
//	and its hit boxes differ between head families. Every placement logs
//	the target's attachments; `probe_head <model>` swaps his head for that
//	one first, so a replay can wear the head a capture's hits read
//	(cod11-turrets.md 13.6):
//
//	PROBE models <entity number> <model>...
swap_head()
{
	want = getcvar("probe_head");
	n = self getattachsize();
	if (want != "" && n > 0)
	{
		//	The head is the character script's first attachment: all of
		//	them come off and go back in order, the first swapped.
		models = [];
		tags = [];
		for (i = 0; i < n; i++)
		{
			models[i] = self getattachmodelname(i);
			tags[i] = self getattachtagname(i);
		}
		self detachall();
		models[0] = want;
		for (i = 0; i < n; i++)
		{
			if (tags[i] == "")
				self attach(models[i]);
			else
				self attach(models[i], tags[i]);
		}
	}
	line = "PROBE models " + self getEntityNumber();
	n = self getattachsize();
	for (i = 0; i < n; i++)
		line = line + " " + self getattachmodelname(i);
	logPrint(line + "\n");
}

//	With `probe_pose 1`: which pose a turret round meets on a body whose own
//	end frame has not run yet (combat doc 16.1). Every hit logs its time,
//	location and point instead of landing; the axis client flips its own view
//	pitch (`--probe-pitch-flip`) and logs the snapshot each flip reached.
watch_pose()
{
	if (getcvar("probe_pose") != "1")
		return;
	wait 1;
	level.callbackPlayerDamage = ::pose_damage;
}

pose_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE hit " + gettime() + " " + self getEntityNumber() + " " + sHitLoc + " " + iDamage + " " + vPoint + "\n");
}

//	With `probe_hitlog 1`: every hit logs its point and direction to the
//	float's last bit before the stock callback lands it. A component prints
//	as <int>:<thousandths>:<millionths of a thousandth>, `%g` keeping too
//	few digits of the float itself:
//
//	PROBE hitfine <time> <entity number> <hit location> <damage> <point> <dir>
//
//	and with `probe_bodylog 1` the struck body's origin and angles:
//
//	PROBE hitbody <time> <entity number> <origin> <angles>
watch_hitlog()
{
	if (getcvar("probe_hitlog") != "1")
		return;
	wait 1;
	level.probe_hit_next = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::hitlog_damage;
}

hitlog_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE hitfine " + gettime() + " " + self getEntityNumber() + " " + sHitLoc + " " + iDamage + " " + fine(vPoint) + " " + fine(vDir) + "\n");
	if (getcvar("probe_bodylog") == "1")
		logPrint("PROBE hitbody " + gettime() + " " + self getEntityNumber() + " " + fine(self.origin) + " " + fine(self.angles) + "\n");
	if (getcvar("probe_hitgrid") == "1")
		self hitgrid(vPoint, vDir);
	self [[level.probe_hit_next]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
}

//	With `probe_hitgrid 1` as well: inside the damage callback, so on the
//	pose the round met (the frame's first pose of the body, cod11-combat.md
//	3), a 9 by 9 grid of `bulletTrace`s (hitCharacters on) along the
//	round's direction, 1.5 units apart across it and centred on its point,
//	each from 48 before the point to 48 past it. A cell is the distance to
//	where the ray stopped on this body, as fine1 prints it, or `-`:
//
//	PROBE hitgrid <time> <entity number> <row> <cell>...
hitgrid(point, dir)
{
	//	The engine has no vectorcross: right is dir x (0, 0, 1), up right x dir.
	right = vectornormalize((dir[1], 0 - dir[0], 0));
	up = (right[1] * dir[2], 0 - right[0] * dir[2], right[0] * dir[1] - right[1] * dir[0]);
	for (i = -4; i <= 4; i++)
	{
		line = "PROBE hitgrid " + gettime() + " " + self getEntityNumber() + " " + i;
		for (j = -4; j <= 4; j++)
		{
			c = point + (right[0] * j * 1.5 + up[0] * i * 1.5, right[1] * j * 1.5 + up[1] * i * 1.5, right[2] * j * 1.5 + up[2] * i * 1.5);
			start = c - (dir[0] * 48, dir[1] * 48, dir[2] * 48);
			tr = bullettrace(start, c + (dir[0] * 48, dir[1] * 48, dir[2] * 48), true, undefined);
			if (!isdefined(tr["entity"]) || tr["entity"] != self || tr["fraction"] >= 1)
				line = line + " -";
			else
				line = line + " " + fine1(tr["fraction"] * 96);
		}
		logPrint(line + "\n");
	}
}

fine(v)
{
	return fine1(v[0]) + "," + fine1(v[1]) + "," + fine1(v[2]);
}

//	The engine has no int(): whole() takes the whole part by halving steps,
//	each subtraction exact, so only the * 1000 products round, well under
//	the float's own step.
fine1(x)
{
	sign = "";
	if (x < 0)
	{
		sign = "-";
		x = 0 - x;
	}
	n = whole(x);
	r = (x - n) * 1000;
	m = whole(r);
	return sign + n + ":" + m + ":" + ((r - m) * 1000000);
}

whole(x)
{
	n = 0;
	for (b = 8192; b >= 1; b = b / 2)
	{
		if (x - n >= b)
			n = n + b;
	}
	return n;
}

//	With `probe_gungrid 1`: every frame the allied gunner is off his
//	placement spot (on the gun), a 5 by 5 grid of `bulletTrace`s
//	(hitCharacters on) along the gun's base forward, 6 units apart across
//	it, rows 30..54 over his origin, each from 40 behind the column to 40
//	past it. A cell is the distance to where the ray stopped on him, as
//	fine1 prints it, or `-`; the rows read the pose his turret anim gave
//	him, the fire leaves included (cod11-turrets.md 13.9):
//
//	PROBE gungrid <time> <entity number> <cell>...
watch_gungrid()
{
	if (getcvar("probe_gungrid") != "1")
		return;
	wait 1;
	gun = probe_gun();
	if (!isdefined(gun))
		return;
	fwd = anglestoforward(gun.angles);
	right = (fwd[1], 0 - fwd[0], 0);
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] gungrid(fwd, right);
		wait 0.05;
	}
}

gungrid(fwd, right)
{
	if (!isdefined(self.probe_spot))
		return;
	if (self.pers["team"] != "allies")
		return;
	if (self.sessionstate != "playing")
		return;
	d = self.origin - self.probe_spot;
	if (d[0] * d[0] + d[1] * d[1] < 16)
		return;
	line = "PROBE gungrid " + gettime() + " " + self getEntityNumber();
	for (i = 0; i < 5; i++)
	{
		for (j = -2; j <= 2; j++)
		{
			c = self.origin + (right[0] * j * 6, right[1] * j * 6, 30 + i * 6);
			start = c - (fwd[0] * 40, fwd[1] * 40, 0);
			tr = bullettrace(start, c + (fwd[0] * 40, fwd[1] * 40, 0), true, undefined);
			if (!isdefined(tr["entity"]) || tr["entity"] != self || tr["fraction"] >= 1)
				line = line + " -";
			else
				line = line + " " + fine1(tr["fraction"] * 80);
		}
	}
	logPrint(line + "\n");
}

//	With `probe_respawn 1`: a second after the allied gunner is on the gun
//	(his origin left the placement spot across the ground), the script spawns him again as a
//	player 40 units further back, facing 90 off the gun, and logs what it
//	reads straight after the `spawn` and a frame later, once per spawn:
//
//	PROBE seat <time> <entity number> <origin> <angles>
//	PROBE seatnext <time> <entity number> <origin> <angles>
watch_respawn()
{
	if (getcvar("probe_respawn") != "1")
		return;
	wait 1;
	gun = probe_gun();
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_respawn(gun);
		wait 0.05;
	}
}

try_respawn(gun)
{
	if (!isdefined(self.probe_spot))
		return;
	if (isdefined(self.probe_respawned))
		return;
	if (self.pers["team"] != "allies")
		return;
	d = self.origin - self.probe_spot;
	if (d[0] * d[0] + d[1] * d[1] < 16)
		return;
	self.probe_respawned = 1;
	self thread respawn(gun);
}

respawn(gun)
{
	self endon("disconnect");
	wait 1;
	fwd = anglestoforward(gun.angles);
	spot = self.probe_spot - (fwd[0] * 40, fwd[1] * 40, 0);
	self spawn(spot, (0, gun.angles[1] + 90, 0));
	logPrint("PROBE seat " + getTime() + " " + self getEntityNumber() + " " + self.origin + " " + self.angles + "\n");
	wait 0.05;
	logPrint("PROBE seatnext " + getTime() + " " + self getEntityNumber() + " " + self.origin + " " + self.angles + "\n");
}
