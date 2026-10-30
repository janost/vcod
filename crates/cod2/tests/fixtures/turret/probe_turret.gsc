// A CoD2 gametype for the turret captures (tools/run_probe_cod2.sh with
// PROBE_GSC pointing here): every client that begins spawns probe_back
// units behind the misc_turret nearest probe_x/y/z, on the floor under that
// spot, facing the gun's yaw, holding the kar98k, and takes no damage.
//
// probe_gun <weapon>: the level first spawns that turret (`spawnTurret`) on
// the floor under probe_x/y/z facing probe_yaw, with probe_gunmodel's
// model (xmodel/weapon_mg42 unset), and the client mans that one (no
// stock map places a prone or duck gun).
// probe_back <units>: how far behind the gun the client spawns (40 unset;
// a gun on the floor needs its 32-unit box and the body's 15 clear).
// probe_die <ms>: that long after the client mounts, it suicides on the
// gun; the killed callback sets the stock dead session and clones the body,
// and the client spawns again behind the gun two seconds later.
// probe_side <units>: with a second client (run_probe_cod2.sh SECOND),
// client 0 spawns that far to the gun's left of the spot behind it, so two
// players stand at one gun.
// probe_shooter <units>: client 0 spawns that far in front of the gun
// instead, holding probe_weapon and looking at the gunner's spot behind the
// gun raised probe_aimz from its own eye (origin + 60); its rounds on the
// gunner go through finishPlayerDamage, the gunner's on it do nothing, and
// a gunner it kills spawns again behind the gun two seconds later.
main()
{
	level.callbackStartGameType = ::startGameType;
	level.callbackPlayerConnect = ::playerConnect;
	level.callbackPlayerDisconnect = ::nothing;
	level.callbackPlayerDamage = ::playerDamage;
	level.callbackPlayerKilled = ::playerKilled;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

startGameType()
{
	precacheItem("kar98k_mp");
	if(getCvar("probe_weapon") != "")
		precacheItem(getCvar("probe_weapon"));
	precacheModel("xmodel/playerbody_german_africa01");
	precacheModel("xmodel/head_german_afrca_christoph");
	precacheModel("xmodel/helmet_german_africa");
	game["state"] = "playing";
	weapon = getCvar("probe_gun");
	if(weapon == "")
		return;
	model = getCvar("probe_gunmodel");
	if(model == "")
		model = "xmodel/weapon_mg42";
	precacheTurret(weapon);
	precacheModel(model);
	at = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	trace = bulletTrace(at + (0, 0, 16), at - (0, 0, 256), false, undefined);
	gun = spawnTurret("misc_turret", trace["position"], weapon);
	gun.angles = (0, getCvarFloat("probe_yaw"), 0);
	gun setModel(model);
	logPrint("PROBE spawnturret " + gun getEntityNumber() + " " + weapon + " " + gun.origin + "\n");
}

nearestTurret(at)
{
	best = undefined;
	guns = getEntArray("misc_turret", "classname");
	for(i = 0; i < guns.size; i++)
	{
		if(!isDefined(best) || distance(guns[i].origin, at) < distance(best.origin, at))
			best = guns[i];
	}
	return best;
}

playerConnect()
{
	self waittill("begin");
	self.sessionteam = "none";
	self.spectatorclient = -1;
	self.archivetime = 0;
	self.psoffsettime = 0;
	self.statusicon = "";
	at = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	gun = nearestTurret(at);
	logPrint("PROBE turret " + gun getEntityNumber() + " " + gun.origin + " " + gun.angles + "\n");
	self thread watchTurret(gun);
	if(getCvar("probe_shooter") != "" && self getEntityNumber() == 0)
		self spawnInFront(gun);
	else
		self spawnBehind(gun);
}

spawnInFront(gun)
{
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	fwd = anglesToForward((0, gun.angles[1], 0));
	origin = onFloor(gun.origin + vectorScale(fwd, getCvarFloat("probe_shooter")));
	behind = onFloor(gun.origin - vectorScale(fwd, 40));
	angles = vectortoangles(behind + (0, 0, getCvarFloat("probe_aimz")) - (origin + (0, 0, 60)));
	self spawn(origin, angles);
	self setPlayerAngles(angles);
	w = getCvar("probe_weapon");
	self setWeaponSlotWeapon("primary", w);
	self giveMaxAmmo(w);
	self setSpawnWeapon(w);
	logPrint("PROBE spawned " + self.origin + "\n");
}

onFloor(spot)
{
	trace = bulletTrace(spot + (0, 0, 16), spot - (0, 0, 256), false, undefined);
	return trace["position"] + (0, 0, 0.125);
}

spawnBehind(gun)
{
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	back = 40;
	if(getCvar("probe_back") != "")
		back = getCvarFloat("probe_back");
	spot = gun.origin - vectorScale(anglesToForward((0, gun.angles[1], 0)), back);
	if(getCvar("probe_side") != "" && self getEntityNumber() == 0)
		spot = spot + vectorScale(anglesToRight((0, gun.angles[1], 0)), 0 - getCvarFloat("probe_side"));
	trace = bulletTrace(spot + (0, 0, 16), spot - (0, 0, 256), false, undefined);
	origin = trace["position"] + (0, 0, 0.125);
	self spawn(origin, (0, gun.angles[1], 0));
	// A player with no model has no server DObj, and `player_die` returns
	// at once without one (lnxded 0x810175a).
	if(getCvarInt("probe_die") > 0 || getCvar("probe_shooter") != "")
	{
		self detachAll();
		self setModel("xmodel/playerbody_german_africa01");
		self attach("xmodel/head_german_afrca_christoph", "", true);
		self attach("xmodel/helmet_german_africa", "", true);
	}
	self giveWeapon("kar98k_mp");
	self setSpawnWeapon("kar98k_mp");
	logPrint("PROBE spawned " + self.origin + "\n");
}

vectorScale(v, s)
{
	return (v[0] * s, v[1] * s, v[2] * s);
}

watchTurret(gun)
{
	self endon("disconnect");
	for(;;)
	{
		gun waittill("trigger", who);
		logPrint("PROBE trigger " + who getEntityNumber() + " " + getTime() + "\n");
		die = getCvarInt("probe_die");
		if(die > 0 && who == self)
			self thread dieOnGun(gun, die);
	}
}

dieOnGun(gun, ms)
{
	self endon("disconnect");
	wait ms / 1000;
	logPrint("PROBE suicide " + getTime() + " " + self.origin + "\n");
	self suicide();
	wait 2;
	self spawnBehind(gun);
}

nothing()
{
}

playerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset)
{
	logPrint("PROBE damage " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + "\n");
	if(getCvar("probe_shooter") != "" && self getEntityNumber() != 0)
		self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset);
}

playerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
{
	logPrint("PROBE killed " + getTime() + " " + sMeansOfDeath + " " + sWeapon + " " + self.origin + "\n");
	self.sessionstate = "dead";
	body = self cloneplayer(deathAnimDuration);
	logPrint("PROBE body " + body getEntityNumber() + " " + body.origin + "\n");
	if(getCvar("probe_shooter") != "")
		self thread respawnBehind();
}

respawnBehind()
{
	self endon("disconnect");
	wait 2;
	at = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	self spawnBehind(nearestTurret(at));
}
