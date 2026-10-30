// probe_moves.gsc with client 0 set probe_dy further along y: the still
// body a second client runs into (cod2-movement.md 20). Same cvars, plus
// probe_dy.
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
	level.probe_weapon = getCvar("probe_weapon");
	if (level.probe_weapon == "")
		level.probe_weapon = "kar98k_mp";
	precacheItem(level.probe_weapon);
	game["state"] = "playing";
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
	origin = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	if (self getEntityNumber() == 0)
		origin = origin + (0, getCvarFloat("probe_dy"), 0);
	self spawn(origin, (0, getCvarFloat("probe_yaw"), 0));
	self giveWeapon(level.probe_weapon);
	self setSpawnWeapon(level.probe_weapon);
	logPrint("PROBE spawned " + self.origin + "\n");
	if (getCvarInt("probe_hurt"))
		self thread hurt();
}

hurt()
{
	self endon("disconnect");
	wait 1.5;
	for (i = 0; i < 26; i++)
	{
		dir = anglesToForward((0, getCvarFloat("probe_yaw") + i * 45, 0));
		self finishPlayerDamage(self, self, 3, 0, "MOD_PISTOL_BULLET", "kar98k_mp", self.origin, dir, "torso_upper", 0);
		logPrint("PROBE hurt " + i + " " + getTime() + "\n");
		wait 0.7;
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
