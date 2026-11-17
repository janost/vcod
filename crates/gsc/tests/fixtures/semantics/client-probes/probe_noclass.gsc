//	What a script reads off a client entity between its connect and its
//	first spawn (cod11-gametypes-re-bel.md 8.3): the fields at the connect
//	callback, three seconds on with no spawn, and right after the spawn;
//	then what `.headiconteam` reads after each of eight strings (7.3).
//	Run: tools/run_probe.sh client-probes/probe_noclass mp_carentan, and
//	vcod --net-probe <server> --probe-secs 10 once it is up.

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
}

Callback_PlayerDisconnect()
{
}

Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
}

Callback_PlayerKilled(eInflictor, attacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc)
{
}

show(v)
{
	if (!isdefined(v))
		return "undef";
	return v;
}

fields(tag)
{
	logPrint("PROBE " + tag + " classname " + show(self.classname) + "\n");
	logPrint("PROBE " + tag + " targetname " + show(self.targetname) + "\n");
	logPrint("PROBE " + tag + " model " + show(self.model) + "\n");
	logPrint("PROBE " + tag + " origin " + show(self.origin) + "\n");
	logPrint("PROBE " + tag + " angles " + show(self.angles) + "\n");
	logPrint("PROBE " + tag + " health " + show(self.health) + "\n");
	logPrint("PROBE " + tag + " maxhealth " + show(self.maxhealth) + "\n");
	logPrint("PROBE " + tag + " sessionstate " + show(self.sessionstate) + "\n");
	logPrint("PROBE " + tag + " sessionteam " + show(self.sessionteam) + "\n");
	logPrint("PROBE " + tag + " score " + show(self.score) + "\n");
	logPrint("PROBE " + tag + " deaths " + show(self.deaths) + "\n");
	logPrint("PROBE " + tag + " statusicon " + show(self.statusicon) + "\n");
	logPrint("PROBE " + tag + " headicon " + show(self.headicon) + "\n");
	logPrint("PROBE " + tag + " headiconteam " + show(self.headiconteam) + "\n");
	logPrint("PROBE " + tag + " spectatorclient " + show(self.spectatorclient) + "\n");
	logPrint("PROBE " + tag + " archivetime " + show(self.archivetime) + "\n");
	logPrint("PROBE " + tag + " alive " + isalive(self) + "\n");
	logPrint("PROBE " + tag + " isplayer " + isplayer(self) + "\n");
	logPrint("PROBE " + tag + " num " + self getEntityNumber() + "\n");
	logPrint("PROBE " + tag + " byplayer " + getentarray("player", "classname").size + "\n");
	logPrint("PROBE " + tag + " bynoclass " + getentarray("noclass", "classname").size + "\n");
}

Callback_PlayerConnect()
{
	fields("connect");
	wait 3;
	fields("wait");
	spawns = getentarray("mp_deathmatch_spawn", "classname");
	self.sessionstate = "playing";
	self spawn(spawns[0].origin, spawns[0].angles);
	fields("spawned");
	// `.headiconteam`'s setter on strings past its four names, read back
	// through the getter (gametypes doc 7.3). The first illegal one is a
	// script error that ends the run: on 1.1 and 1.5 "spectator", on UO
	// any string but the four.
	team("none");
	team("allies");
	team("axis");
	team("foo");
	team("Axis");
	team("");
	team("ALLIES");
	team("spectator");
}

team(s)
{
	self.headiconteam = s;
	logPrint("PROBE team '" + s + "' " + self.headiconteam + "\n");
}
