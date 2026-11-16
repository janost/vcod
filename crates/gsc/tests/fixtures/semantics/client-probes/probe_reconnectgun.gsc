//	A client that reconnects into its held slot while on a mounted gun
//	(`SV_DirectConnect`'s reconnect arm; on 1.1 no `ClientDisconnect`
//	runs): each connect is spawned behind mp_carentan's gun at
//	(1712 1830 8) and mounted on it through `useby` a second after it
//	begins. Paired with a `--probe-edge reconnect` client, whose `EDGE
//	snap` lines carry the mount fields. See this directory's README.md,
//	"probe_reconnectgun".

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType()
{
	level.gen = 0;
	guns = getentarray("misc_mg42", "classname");
	for (i = 0; i < guns.size; i++)
		if (!isdefined(level.gun) || distance(guns[i].origin, (1712, 1830, 8)) < distance(level.gun.origin, (1712, 1830, 8)))
			level.gun = guns[i];
	fwd = anglestoforward((0, level.gun.angles[1], 0));
	s = level.gun.origin - (fwd[0] * 40, fwd[1] * 40, 0);
	t = bulletTrace(s + (0, 0, 32), s - (0, 0, 200), false, undefined);
	level.site = t["position"] + (0, 0, 0.125);
	logPrint("PROBE gun " + level.gun getEntityNumber() + " " + level.gun.origin + " site " + level.site + "\n");
}

Callback_PlayerConnect()
{
	gen = level.gen;
	level.gen++;
	num = self getEntityNumber();
	logPrint("PROBE connect " + num + " gen " + gen + " t " + getTime() + "\n");
	self waittill("begin");
	self.sessionteam = "allies";
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(level.site, (0, level.gun.angles[1], 0));
	wait 1;
	level.gun useby(self);
	logPrint("PROBE useby " + num + " gen " + gen + " t " + getTime() + "\n");
}

Callback_PlayerDisconnect()
{
	logPrint("PROBE disconnect_callback " + self getEntityNumber() + " t " + getTime() + "\n");
}

Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
