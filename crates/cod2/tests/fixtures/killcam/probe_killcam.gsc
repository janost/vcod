// A CoD2 gametype for the killcam capture (tools/run_probe_cod2.sh with
// PROBE_GSC pointing here): probe_combat.gsc's two clients, the target
// mortal, and on its death stock dm's tail: `sessionstate` "dead", a
// clone, `wait 2`, then the stock `_killcam::killcam` on the shooter
// (`init` turns the archive on). Logs the archivetime the engine trimmed
// and the killcam's end. With `probe_killerdies` set the killer drops his
// helmet 0.5 s after the kill and suicides 0.5 s later, inside the
// killcam's replay, then respawns `probe_respawn` seconds after that.
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
	precacheItem(primaryWeapon());
	if(getCvar("probe_weapon2") != "")
		precacheItem(getCvar("probe_weapon2"));
	precacheItem("frag_grenade_american_mp");
	precacheModel("xmodel/playerbody_german_africa01");
	precacheModel("xmodel/head_german_afrca_christoph");
	precacheModel("xmodel/helmet_german_africa");
	game["state"] = "playing";
	level.splitscreen = isSplitScreen();
	setCvar("probe_mortal", "1");
	thread maps\mp\gametypes\_killcam::init();
}

primaryWeapon()
{
	w = getCvar("probe_weapon");
	if(w == "")
		w = "kar98k_mp";
	return w;
}

playerConnect()
{
	self waittill("begin");
	self spawnPlayer();
}

spawnPlayer()
{
	self.sessionteam = "none";
	self.sessionstate = "playing";
	self.spectatorclient = -1;
	self.archivetime = 0;
	self.psoffsettime = 0;
	self.statusicon = "";
	self.maxhealth = 100;
	self.health = 100;
	target = (getCvarFloat("probe_vx"), getCvarFloat("probe_vy"), getCvarFloat("probe_vz"));
	if(getCvar("probe_vx") != "" && self getEntityNumber() == 0)
	{
		self spawn(target, (0, getCvarFloat("probe_vyaw"), 0));
		self setWeaponSlotWeapon("primary", primaryWeapon());
		self setSpawnWeapon(primaryWeapon());
	}
	else
	{
		origin = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
		angles = (0, getCvarFloat("probe_yaw"), 0);
		if(getCvar("probe_vx") != "")
			angles = vectortoangles(target + (0, 0, getCvarFloat("probe_aimz")) - (origin + (0, 0, 60)));
		self spawn(origin, angles);
		self setPlayerAngles(angles);
		w = primaryWeapon();
		self setWeaponSlotWeapon("primary", w);
		self giveMaxAmmo(w);
		if(getCvar("probe_weapon2") != "")
		{
			self setWeaponSlotWeapon("primaryb", getCvar("probe_weapon2"));
			self giveMaxAmmo(getCvar("probe_weapon2"));
		}
		self giveWeapon("frag_grenade_american_mp");
		self setWeaponClipAmmo("frag_grenade_american_mp", 3);
		self switchToOffhand("frag_grenade_american_mp");
		self setSpawnWeapon(w);
	}
	// mp_german_africa's body with its first head: the hit boxes a
	// random head would move.
	self detachAll();
	self setModel("xmodel/playerbody_german_africa01");
	self attach("xmodel/head_german_afrca_christoph", "", true);
	self attach("xmodel/helmet_german_africa", "", true);
	logPrint("PROBE spawned " + self getEntityNumber() + " " + self.origin + "\n");
}

nothing()
{
}

playerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset)
{
	logPrint("PROBE damage " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + iDFlags + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + " " + self.health + " " + vPoint + " " + vDir + "\n");
	if(!getCvarInt("probe_mortal"))
	{
		self.health = 100;
		if(iDamage > 99)
			iDamage = 99;
	}
	self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset);
}

playerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
{
	self endon("spawned");
	logPrint("PROBE killed " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + " " + timeOffset + "\n");
	obituary(self, eAttacker, sWeapon, sMeansOfDeath);
	self.sessionstate = "dead";
	body = self cloneplayer(deathAnimDuration);
	if(eAttacker == self)
	{
		wait getCvarFloat("probe_respawn");
		self spawnPlayer();
		return;
	}
	if(getCvar("probe_killerdies") != "")
		eAttacker thread killerDies();
	wait 2;
	logPrint("PROBE killcam " + self getEntityNumber() + " " + getTime() + "\n");
	self thread logTrim();
	self maps\mp\gametypes\_killcam::killcam(eAttacker getEntityNumber(), 2, timeOffset, true);
	logPrint("PROBE killcam_end " + self getEntityNumber() + " " + getTime() + " " + self.sessionstate + "\n");
}

logTrim()
{
	wait 0.05;
	logPrint("PROBE archivetime " + self getEntityNumber() + " " + getTime() + " " + self.archivetime + "\n");
}

killerDies()
{
	wait 0.5;
	self detach("xmodel/helmet_german_africa", "");
	logPrint("PROBE detach " + self getEntityNumber() + " " + getTime() + "\n");
	wait 0.5;
	self suicide();
}
