// probe_turret.gsc with an mp40 beside the gun, for the use list's check
// (cod2-gsc-diff.md 5.3): every client spawns 40 units behind the
// misc_turret nearest probe_x/y/z facing along it, and with probe_item 1
// (-1) an mp40 hangs 30 units ahead of (behind) him at eye height.
//   PROBE_GSC=crates/cod2/tests/fixtures/turret/probe_usehint.gsc \
//   SRV="+set probe_item 1" tools/run_probe_cod2.sh turret mp_carentan 1718 1824 5 0 <out>
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
	precacheItem("mp40_mp");
	game["state"] = "playing";
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
	self.sessionstate = "playing";
	self.spectatorclient = -1;
	self.archivetime = 0;
	self.psoffsettime = 0;
	self.statusicon = "";
	self.maxhealth = 100;
	self.health = 100;
	at = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	gun = nearestTurret(at);
	spot = gun.origin - vectorScale(anglesToForward((0, gun.angles[1], 0)), 40);
	trace = bulletTrace(spot + (0, 0, 16), spot - (0, 0, 256), false, undefined);
	origin = trace["position"] + (0, 0, 0.125);
	logPrint("PROBE turret " + gun getEntityNumber() + " " + gun.origin + " " + gun.angles + "\n");
	self spawn(origin, (0, gun.angles[1], 0));
	self giveWeapon("kar98k_mp");
	self setSpawnWeapon("kar98k_mp");
	logPrint("PROBE spawned " + self.origin + "\n");
	self thread watchTurret(gun);
	side = getCvarFloat("probe_item");
	if(side != 0)
	{
		pos = self.origin + (0, 0, 60) + vectorScale(anglesToForward((0, gun.angles[1], 0)), 30 * side);
		item = spawn("weapon_mp40_mp", pos, 1);
		logPrint("PROBE item " + item getEntityNumber() + " " + item.origin + "\n");
	}
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
		logPrint("PROBE trigger " + who getEntityNumber() + "\n");
	}
}

nothing()
{
}

playerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset)
{
	logPrint("PROBE damage " + iDamage + " " + sMeansOfDeath + "\n");
}

playerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
{
}
