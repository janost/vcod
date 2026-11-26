//	United Offensive: what a vehicle's death does to its riders
//	(`Scr_Vehicle_Die`, docs/research/coduo-vehicles.md 11.2). Stock tdm
//	with the map's vehicles. Client 0 is put `probe_side` units (default
//	110) off the right flank
//	of the first vehicle (of the `probe_vehicletype` cvar's type when set)
//	and client 1 off its left, facing the hull, so a `--probe-vehicle
//	driver` and a `--probe-vehicle gunner` client press use into seats 1
//	and 2. A frame after the second seat is taken (the first, with
//	`probe_riders` 1) the vehicle takes
//	`dodamage(10000, origin + (0, 0, 1), by, by, "MOD_EXPLOSIVE")`, `by`
//	the vehicle itself (`probe_die_by` unset or `self`) or client 1
//	(`probe_die_by` `gunner`), seated or not. The damage callback logs every call before
//	it runs tdm's. Logs, each as `PROBE <step> <time> ...`:
//
//	place      where a player was put
//	activated  a seat taken: vehicle, seat, player
//	kill       the vehicle, its health, the attacker
//	rider      a rider's number, `isInVehicle`, health and origin, with
//	           `when`: before the hit, straight after it in the killing
//	           thread, a frame later, half a second later and 1.5 s later
//	dmg        a damage callback: victim, inflictor, attacker, damage,
//	           flags, means, weapon, point, direction, hit location, the
//	           victim's health and `isInVehicle` on the way in
//	deact      a rider's `vehicle_deactivated`: the player
//	death      the vehicle's `death`
//	freed      with `probe_kill` `free`: `freeVehicle()` in place of the
//	           damage, its `death` as a level thread sees it; `free_quiet`
//	           first ends the stock scripts' rider death threads
//
//	Stock tdm reads `level.teamkill_penalty`, which nothing sets, when a
//	player dies to something not a player; `_tankdrive_gmi`'s
//	`tank_death_player_damage` kills a rider so when the tank is its own
//	attacker, and the read is a fatal script error on retail. The probe sets
//	it to 0 first.
//
//	UO only. The capture recipe is in the fixture header.

main()
{
	thread watch_places();
	thread log_vehicle();
	maps\mp\gametypes\tdm::main();
	level.probe_tdm_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::probe_damage;
}

num(e)
{
	if (!isdefined(e))
		return "undefined";
	return e getEntityNumber();
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	point = "undefined";
	if (isdefined(vPoint))
		point = vPoint;
	dir = "undefined";
	if (isdefined(vDir))
		dir = vDir;
	logPrint("PROBE dmg " + gettime() + " " + self getEntityNumber() + " inf " + num(eInflictor) + " att " + num(eAttacker) + " " + iDamage + " flags " + iDFlags + " " + sMeansOfDeath + " " + sWeapon + " " + point + " " + dir + " " + sHitLoc + " health " + self.health + " veh " + self isInVehicle() + "\n");
	[[level.probe_tdm_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
}

//	The vehicle the probe uses: the lowest-numbered `script_vehicle`, of
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
		return;
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place(veh);
		wait 0.05;
	}
}

//	Early returns: sessionstate is undefined before the first spawn.
try_place(veh)
{
	if (!isdefined(self.sessionstate) || self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_placed) || self getEntityNumber() >= 2)
		return;
	self.probe_placed = 1;
	side = getcvarint("probe_side");
	if (side == 0)
		side = 110;
	if (self getEntityNumber() != 0)
		side = 0 - side;
	right = anglestoright(veh.angles);
	spot = veh.origin + (right[0] * side, right[1] * side, 8);
	yaw = vectortoangles(veh.origin - spot)[1];
	self setorigin(spot);
	self setplayerangles((10, yaw, 0));
	logPrint("PROBE place " + gettime() + " " + self getEntityNumber() + " " + spot + " " + yaw + "\n");
}

log_vehicle()
{
	wait 1;
	veh = probe_vehicle();
	if (!isdefined(veh))
	{
		logPrint("PROBE vehicle unsupported " + getcvar("mapname") + "\n");
		return;
	}
	logPrint("PROBE vehicle " + veh getEntityNumber() + " " + veh.vehicletype + " " + veh.origin + " " + veh.angles + "\n");
	if (!isdefined(level.teamkill_penalty))
		level.teamkill_penalty = 0;
	veh thread log_death();
	want = getcvarint("probe_riders");
	if (want == 0)
		want = 2;
	riders = [];
	for (;;)
	{
		veh waittill("activated", vehpos, activator);
		logPrint("PROBE activated " + gettime() + " " + veh getEntityNumber() + " seat " + vehpos + " by " + activator getEntityNumber() + "\n");
		riders[riders.size] = activator;
		activator thread log_deact();
		if (riders.size == want)
			break;
	}
	wait 0.05;
	by = veh;
	if (getcvar("probe_die_by") == "gunner")
		by = player(1);
	log_riders(riders, "before");
	kill = getcvar("probe_kill");
	if (kill == "free_quiet")
	{
		// The stock vehicle scripts' rider thread (`death_player_damage`,
		// `tank_death_player_damage`) ends on this; on a freed vehicle's
		// `death` it reads a field off the dead handle.
		for (i = 0; i < riders.size; i++)
			riders[i] notify("deactivated_player");
		kill = "free";
	}
	if (kill == "free")
	{
		level thread log_free(veh);
		logPrint("PROBE kill " + gettime() + " " + veh getEntityNumber() + " " + veh.health + " free\n");
		veh freeVehicle();
	}
	else
	{
		logPrint("PROBE kill " + gettime() + " " + veh getEntityNumber() + " " + veh.health + " by " + by getEntityNumber() + "\n");
		veh dodamage(10000, veh.origin + (0, 0, 1), by, by, "MOD_EXPLOSIVE");
	}
	log_riders(riders, "after");
	wait 0.05;
	log_riders(riders, "frame");
	wait 0.5;
	log_riders(riders, "later");
	wait 1;
	log_riders(riders, "late");
	logPrint("PROBE end " + gettime() + "\n");
}

//	`freeVehicle`'s `death`, seen from a level thread: the vehicle's own
//	threads never run again (docs/research/coduo-vehicles.md 17.4).
log_free(veh)
{
	veh waittill("death");
	logPrint("PROBE freed " + gettime() + "\n");
}

player(n)
{
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
		if (players[i] getEntityNumber() == n)
			return players[i];
	return undefined;
}

log_riders(riders, when)
{
	for (i = 0; i < riders.size; i++)
	{
		r = riders[i];
		logPrint("PROBE rider " + gettime() + " " + when + " " + r getEntityNumber() + " veh " + r isInVehicle() + " health " + r.health + " " + r.origin + "\n");
	}
}

log_deact()
{
	for (;;)
	{
		self waittill("vehicle_deactivated", veh);
		logPrint("PROBE deact " + gettime() + " " + self getEntityNumber() + " " + num(veh) + "\n");
	}
}

log_death()
{
	self waittill("death", attacker);
	logPrint("PROBE death " + gettime() + " " + num(attacker) + "\n");
}
