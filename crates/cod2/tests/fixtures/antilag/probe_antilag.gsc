// A CoD2 gametype for the antilag capture (tools/run_probe_cod2.sh with
// PROBE_GSC pointing here, SECOND=weave and PROBE_ARGS='--lag 100'):
// probe_hitpose.gsc's pair with the frame archive on (`setarchive(true)`,
// what lets `G_AntiLagRewindClientPos` find a past), the target strafing
// and the shooter's cmd clock held 100 ms back, a ppsh's trigger held
// while the view sweeps the target's live origin. Each `PROBE damage`
// line ends with the victim's and the attacker's `origin` as the
// callback reads them; each frame's `PROBE at` line is the target's
// origin outside any shot.
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
	setarchive(true);
	level thread watch();
}

// The target's origin once a frame, between shots.
watch()
{
	for(;;)
	{
		players = getEntArray("player", "classname");
		for(i = 0; i < players.size; i++)
			if(players[i] getEntityNumber() == 0)
				logPrint("PROBE at " + getTime() + " " + players[i].origin + "\n");
		wait 0.05;
	}
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
	self.sessionteam = "none";
	self.sessionstate = "playing";
	self.spectatorclient = -1;
	self.archivetime = 0;
	self.psoffsettime = 0;
	self.statusicon = "";
	self.maxhealth = 100;
	self.health = 100;
	target = (getCvarFloat("probe_vx"), getCvarFloat("probe_vy"), getCvarFloat("probe_vz"));
	origin = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	if(getCvar("probe_turret") != "")
	{
		gun = nearestTurret(origin);
		fwd = anglesToForward((0, gun.angles[1], 0));
		origin = onFloor(gun.origin - vectorScale(fwd, 40));
		target = onFloor(gun.origin + vectorScale(fwd, getCvarFloat("probe_turret")));
		logPrint("PROBE turret " + gun getEntityNumber() + " " + gun.origin + " " + gun.angles + "\n");
	}
	if((getCvar("probe_vx") != "" || getCvar("probe_turret") != "") && self getEntityNumber() == 0)
	{
		self spawn(target, (0, getCvarFloat("probe_vyaw"), 0));
		self setWeaponSlotWeapon("primary", primaryWeapon());
		self setSpawnWeapon(primaryWeapon());
	}
	else
	{
		angles = (0, getCvarFloat("probe_yaw"), 0);
		if(getCvar("probe_vx") != "" || getCvar("probe_turret") != "")
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
		self thread sweep(target);
	}
	// mp_german_africa's body with its first head: the hit boxes a
	// random head would move.
	self detachAll();
	self setModel("xmodel/playerbody_german_africa01");
	self attach("xmodel/head_german_afrca_christoph", "", true);
	self attach("xmodel/helmet_german_africa", "", true);
	logPrint("PROBE spawned " + self getEntityNumber() + " " + self.origin + "\n");
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

onFloor(spot)
{
	trace = bulletTrace(spot + (0, 0, 16), spot - (0, 0, 256), false, undefined);
	return trace["position"] + (0, 0, 0.125);
}

vectorScale(v, s)
{
	return (v[0] * s, v[1] * s, v[2] * s);
}

nothing()
{
}

playerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset)
{
	// Logged and dropped: no damage, no knockback, no pain anim, so the
	// target's pose is its own cmds' alone. `self.origin` is
	// `r.currentOrigin`, which the rewind moves.
	logPrint("PROBE damage " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + iDFlags + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + " " + self.health + " " + vPoint + " " + vDir + " " + timeOffset + " " + self.origin + " " + eAttacker.origin + "\n");
}

playerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
{
	logPrint("PROBE killed " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + "\n");
	obituary(self, eAttacker, sWeapon, sMeansOfDeath);
}

// The shooter's view, one spot on the target a frame. The target's
// origin is read each frame: it moves.
sweep(target)
{
	victim = undefined;
	for(n = 0; ; n++)
	{
		players = getEntArray("player", "classname");
		for(i = 0; i < players.size; i++)
			if(players[i] getEntityNumber() == 0)
				victim = players[i];
		if(isDefined(victim))
			target = victim.origin;
		eye = self.origin + (0, 0, 60);
		across = anglesToRight(vectorToAngles(target - eye));
		z = 4 + (n * 7) % 69;
		a = (n * 5) % 25 - 12;
		spot = target + (0, 0, z) + vectorScale(across, a);
		self setPlayerAngles(vectorToAngles(spot - eye));
		wait 0.05;
	}
}
