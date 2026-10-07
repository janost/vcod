//	Which queued server commands `SV_AddServerCommand` (cod_lnxded
//	0x808b680) squashes or drops. Needs one plain `--net-probe` client; its
//	`serverCommand` lines are the measurement, the `PROBE step` lines say
//	what each burst queued. See this directory's README.md, "probe_squash".

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

Callback_PlayerConnect()
{
	// Not yet active: a type-0 print should be dropped, a cvar kept.
	self iprintln("sq early print");
	self setClientCvar("sq_early", "1");
	self waittill("begin");
	self.sessionteam = "allies";
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	sp = spawns[0];
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(sp.origin, sp.angles);
	logPrint("PROBE begin\n");
	self thread bursts();
}

step(name)
{
	logPrint("PROBE step " + name + "\n");
	wait 2;
}

bursts()
{
	wait 3;
	// One frame: same cvar twice, two other cvars, a print pair, two bare
	// closeMenus, then the first cvar again behind a print.
	self setClientCvar("sq_a", "1");
	self setClientCvar("sq_a", "2");
	self setClientCvar("sq_b", "1");
	self setClientCvar("sq_c", "1");
	self iprintln("sq dup");
	self iprintln("sq dup");
	self closeMenu();
	self closeMenu();
	self setClientCvar("sq_d", "1");
	self iprintln("sq between");
	self setClientCvar("sq_d", "2");
	step("same_frame");
	// Two frames apart: the first went out with a snapshot before the
	// second was queued.
	self setClientCvar("sq_a", "3");
	wait 0.5;
	self setClientCvar("sq_a", "4");
	step("frames_apart");
	logPrint("PROBE done\n");
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
