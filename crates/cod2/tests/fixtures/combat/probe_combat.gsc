// A CoD2 gametype for the combat captures (tools/run_probe_cod2.sh with
// GT=combat). Every client spawns at once and plays; nothing respawns.
//
// One client: the shooter at probe_x/y/z facing probe_yaw. With probe_vx
// set, client 0 is the target (holding probe_weapon, never firing), at
// probe_vx/vy/vz facing probe_vyaw, and the shooter's spawn view looks at
// the target's origin raised probe_aimz from its own eye (origin + 60).
// Both wear mp_german_africa's body, first head and helmet.
//
// With probe_turret set, both spots come off the misc_turret nearest
// probe_x/y/z instead: the shooter 40 units behind it facing its yaw, the
// target probe_turret units in front of it, each on the floor below.
//
// The shooter holds probe_weapon (primary, default kar98k_mp),
// probe_weapon2 in primaryb when set, and three American frags in the
// offhand. Each damage callback logs a PROBE line; the damage is capped at
// 99 and the health set back to 100 first, so nobody dies, unless
// probe_mortal is 1, when the stock damage runs and a death logs and
// raises the obituary.
// With probe_noknock 1 the damage carries DFLAGS_NO_KNOCKBACK (4): the
// target stands where it spawned.
//
// With probe_fan 1 each damage callback first logs a fan of bulletTraces
// across the struck body: from 40 units in front (+y) to 40 behind, on a
// 2-unit grid of x -16..16 and z 20..70 about the truncated origin, each the
// depth where the trace stops on the target, or `-`. Retail's stops on the
// body's capsule, not its bones (cod2-combat.md 9.3).
//
// With probe_tags 1 the target's bones are logged every frame (tags()).
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
	}
	// mp_german_africa's body with its first head: the hit boxes a
	// random head would move.
	self detachAll();
	self setModel("xmodel/playerbody_german_africa01");
	self attach("xmodel/head_german_afrca_christoph", "", true);
	self attach("xmodel/helmet_german_africa", "", true);
	logPrint("PROBE spawned " + self getEntityNumber() + " " + self.origin + "\n");
	if(getCvarInt("probe_tags") && self getEntityNumber() == 0)
		self thread tags();
}

// probe_tags 1: script_origins linked to the target's bones, logged each
// frame from 8500 to 10000 less probe_vx/vy/vz. A link moves in the entity
// pass, so a line at a frame's time holds the bones the frame before posed.
tags()
{
	names = [];
	names[0] = "j_wrist_le";
	names[1] = "j_elbow_le";
	names[2] = "j_shoulder_le";
	names[3] = "j_spine4";
	names[4] = "j_mainroot";
	names[5] = "j_knee_le";
	names[6] = "j_ankle_le";
	names[7] = "j_wrist_ri";
	// The DObj a link reads is built at the spawn's end frame.
	wait 0.1;
	marks = [];
	for(i = 0; i < names.size; i++)
	{
		marks[i] = spawn("script_origin", self.origin);
		marks[i] linkTo(self, names[i], (0, 0, 0), (0, 0, 0));
	}
	base = (getCvarFloat("probe_vx"), getCvarFloat("probe_vy"), getCvarFloat("probe_vz"));
	for(;;)
	{
		wait 0.05;
		t = getTime();
		if(t < 8500 || t > 10000)
			continue;
		line = "";
		for(i = 0; i < marks.size; i++)
			line = line + " " + (marks[i].origin - base);
		logPrint("PROBE tags " + t + line + "\n");
	}
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
	logPrint("PROBE damage " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + iDFlags + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + " " + self.health + " " + vPoint + " " + vDir + "\n");
	if(getCvarInt("probe_fan"))
		self fan();
	if(!getCvarInt("probe_mortal"))
	{
		self.health = 100;
		if(iDamage > 99)
			iDamage = 99;
	}
	if(getCvarInt("probe_noknock") && !(iDFlags & 4))
		iDFlags += 4;
	self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset);
}

fan()
{
	base = (int(self.origin[0]), int(self.origin[1]), int(self.origin[2]));
	logPrint("PROBE fanat " + getTime() + " " + base + "\n");
	for(z = 20; z <= 70; z += 2)
	{
		row = "";
		for(x = -16; x <= 16; x += 2)
		{
			trace = bulletTrace(base + (x, 40, z), base + (x, -40, z), true, undefined);
			if(isDefined(trace["entity"]) && trace["entity"] == self)
				row = row + " " + trace["fraction"] * 80;
			else
				row = row + " -";
		}
		logPrint("PROBE fan " + getTime() + " " + z + row + "\n");
	}
}

playerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
{
	logPrint("PROBE killed " + self getEntityNumber() + " " + getTime() + " " + iDamage + " " + sMeansOfDeath + " " + sWeapon + " " + sHitLoc + "\n");
	obituary(self, eAttacker, sWeapon, sMeansOfDeath);
}
