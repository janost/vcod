//	What `G_GetNonPVSFriendlyInfo` packs into `iCompassFriendInfo`. Needs
//	two `--net-probe --probe-compass` clients: slot 0 stands at a spawn as
//	the viewer, slot 1 is its allied teammate and is moved about. Each
//	`PROBE step` line says where slot 1 is and what it is; the viewer's
//	`COMPASS` lines are the measurement. See this directory's README.md,
//	"probe_compass".

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
	level.joined = 0;
}

Callback_PlayerConnect()
{
	self waittill("begin");
	n = self getEntityNumber();
	self.sessionteam = "allies";
	level.spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	sp = level.spawns[n];
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(sp.origin, sp.angles);
	logPrint("PROBE begin " + n + " at " + sp.origin + " yaw " + sp.angles[1] + "\n");
	level.joined++;
	if(n == 1)
		self thread wander();
}

step(name)
{
	logPrint("PROBE step " + name + " origin " + self.origin + " angles " + self.angles + "\n");
	wait 1.5;
}

wander()
{
	while(level.joined < 2)
		wait 0.5;
	wait 2;
	for(i = 2; i < level.spawns.size && i < 14; i++)
	{
		self setOrigin(level.spawns[i].origin);
		self setPlayerAngles(level.spawns[i].angles);
		step("spawn" + i);
	}
	self pingPlayer();
	step("ping");
	wait 2;
	self.sessionteam = "axis";
	step("other_team");
	self.sessionteam = "allies";
	step("back_allies");
	self suicide();
	step("suicide_playing");
	// What a stock Callback_PlayerKilled does next.
	self.sessionstate = "dead";
	step("dead_session");
	self.sessionstate = "spectator";
	self spawn(level.spawns[3].origin, level.spawns[3].angles);
	step("spectator_session");
	logPrint("PROBE done\n");
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
