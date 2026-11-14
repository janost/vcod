//	`suicide()` on a player whose session stays `playing`: what
//	`ClientEndFrame` does with a body at health 0 that is not `dead`
//	(docs/research/cod11-combat.md 5.1, "A suicide that stays playing").
//	Two `--net-probe --probe-compass` clients on mp_harbor: slot 0 watches,
//	slot 1 suicides, stays playing, then takes the dead session and spawns
//	again. Each `PROBE step` line carries slot 1's health and whether a
//	trace from slot 0's eye stops on it; the `COMPASS` lines carry each
//	client's `pm_type` and the players its snapshot holds. See this
//	directory's README.md, "probe_suicide".

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
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	//	`probe_suicide_near 1` stands slot n 64 n units off the first spawn,
	//	for a map whose first two spawns share no PVS.
	if(getcvar("probe_suicide_near") == "1")
		self spawn(level.spawns[0].origin + (64 * n, 0, 0), level.spawns[0].angles);
	else
		self spawn(level.spawns[n].origin, level.spawns[n].angles);
	logPrint("PROBE begin " + n + " at " + self.origin + "\n");
	if(n == 0)
		level.watcher = self;
	level.joined++;
	if(n == 1)
		self thread run();
}

step(name)
{
	eye = level.watcher.origin + (0, 0, 60);
	trace = bullettrace(eye, self.origin + (0, 0, 40), true, level.watcher);
	hit = "none";
	if(isDefined(trace["entity"]))
		hit = trace["entity"] getEntityNumber();
	logPrint("PROBE step " + name + " health " + self.health + " state " + self.sessionstate + " trace " + hit + " origin " + self.origin + "\n");
	wait 1.5;
}

run()
{
	while(level.joined < 2)
		wait 0.5;
	wait 2;
	step("alive");
	self suicide();
	step("suicide_playing");
	step("suicide_playing2");
	self.sessionstate = "dead";
	step("dead_session");
	self.sessionstate = "playing";
	self.health = 100;
	self spawn(level.spawns[1].origin, level.spawns[1].angles);
	step("respawned");
	logPrint("PROBE done\n");
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, attacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc)
{
	logPrint("PROBE killed " + self getEntityNumber() + " " + sMeansOfDeath + " health " + self.health + "\n");
}
