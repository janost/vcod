//	United Offensive only: getTurretHeat() and getTurretOverheating() on
//	mp_carentan's turret at (1712 1830 8), whose builtins 1.1 and 1.5 do
//	not compile (coduo-diff.md 21.3.1). Every allied player is put 40 units
//	behind the gun facing along it, every axis player 300 in front facing
//	back; a hit lands no damage. Each frame either value changes it logs
//
//	PROBE heat <time> <heat> <overheating>
//
//	Run by tools/run_probe.sh with a --save-turret client (allies) and a
//	--probe-team axis client.

main()
{
	thread watch_teleports();
	thread watch_heat();
	maps\mp\gametypes\dm::main();
}

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

watch_teleports()
{
	wait 1;
	level.callbackPlayerDamage = ::no_damage;
	gun = probe_gun();
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place(gun);
		wait 0.05;
	}
}

//	Early returns: sessionstate is undefined before the first spawn.
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
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
	logPrint("PROBE place " + self getEntityNumber() + " " + self.pers["team"] + " " + spot + " " + yaw + " " + getTime() + "\n");
}

no_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
}

watch_heat()
{
	wait 1;
	gun = probe_gun();
	last = "";
	for (;;)
	{
		now = gun getturretheat() + " " + gun getturretoverheating();
		if (now != last)
			logPrint("PROBE heat " + gettime() + " " + now + "\n");
		last = now;
		wait 0.05;
	}
}
