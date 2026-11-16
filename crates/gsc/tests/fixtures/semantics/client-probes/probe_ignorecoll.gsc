//	`attach`'s third argument, ignoreCollision, on 1.5b and UO
//	(cod15-protocol-diff.md, `getAttachIgnoreCollision`): one plain
//	`--net-probe` client is spawned facing +x on mp_carentan with the
//	airborne body, `xmodel/basehead_01` and `xmodel/USAirborneHelmet`, the
//	only parts with hit boxes above the shoulders being those two
//	attachments (cod11-combat.md 3.1, 3.6). Six phases re-attach them with
//	the flag off and on in turn, the head alternating with
//	`xmodel/basehead2_02` so each phase's model list differs from the last
//	but "unflagged", which repeats "both"'s list without the flags; each runs level bulletTraces with
//	hitCharacters across the head, 64 units either side of the origin
//	along x, rows a unit apart from 50 to 75 up and columns at y -3, 0 and
//	3. See this directory's README.md, "probe_ignorecoll".

main()
{
	precacheModel("xmodel/playerbody_american_airborne");
	precacheModel("xmodel/basehead_01");
	precacheModel("xmodel/basehead2_02");
	precacheModel("xmodel/USAirborneHelmet");
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType() {}

Callback_PlayerConnect()
{
	self waittill("begin");
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	if (!spawns.size)
		spawns = getentarray("mp_tdm_spawn", "classname");
	self.sessionteam = "allies";
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(spawns[0].origin, (0, 0, 0));
	self setModel("xmodel/playerbody_american_airborne");
	wait 2;
	o = self.origin;
	logPrint("PROBE origin " + o + "\n");
	a = "xmodel/basehead_01";
	b = "xmodel/basehead2_02";
	phase(o, "none", a, false, false);
	phase(o, "head", b, true, false);
	phase(o, "helmet", a, false, true);
	phase(o, "both", b, true, true);
	phase(o, "unflagged", b, false, false);
	phase(o, "none_again", a, false, false);
	logPrint("PROBE done\n");
}

phase(o, name, model, head, helmet)
{
	self detachAll();
	self attach(model, "", head);
	self attach("xmodel/USAirborneHelmet", "", helmet);
	wait 0.2;
	for (z = 50; z <= 75; z++)
	{
		line = "PROBE " + name + " " + z;
		for (y = -3; y <= 3; y += 3)
		{
			s = o + (64, y, z);
			t = bulletTrace(s, o + (-64, y, z), true, undefined);
			e = -1;
			if (isdefined(t["entity"]))
				e = t["entity"] getEntityNumber();
			line = line + " " + e + ":" + t["fraction"];
		}
		logPrint(line + "\n");
	}
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
