//	What `sayAll`, `sayTeam` and `pingPlayer` put on the wire, and how
//	G_Say's speaker and recipient rules treat a dead, zero-health or
//	spectating speaker. Needs two plain `--net-probe` clients: slot 0 speaks
//	as allies, slot 1 listens as axis. Each `PROBE step` line names the
//	`h`/`i` lines the two probes' `CHAT` output should hold; see this
//	directory's README.md, "probe_say".

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
	precacheString(&"GAME_AXIS");
	precacheString(&"MPSCRIPT_WINS");
	precacheString(&"QUICKMESSAGE_FOLLOW_ME");
	level.joined = 0;
}

Callback_PlayerConnect()
{
	self waittill("begin");
	n = self getEntityNumber();
	if(n == 0)
		self.sessionteam = "allies";
	else
		self.sessionteam = "axis";
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	sp = spawns[n];
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(sp.origin, sp.angles);
	logPrint("PROBE begin " + n + "\n");
	level.joined++;
	if(n == 0)
		self thread speak();
}

step(name)
{
	logPrint("PROBE step " + name + "\n");
	wait 2;
}

speak()
{
	while(level.joined < 2)
		wait 0.5;
	wait 3;
	self sayAll("plain words");
	step("sayall_plain");
	self sayAll(&"GAME_AXIS");
	step("sayall_key");
	self sayAll(&"MPSCRIPT_WINS", self);
	step("sayall_key_player");
	self sayAll("one", 2, "three");
	step("sayall_parts");
	self sayTeam(&"QUICKMESSAGE_FOLLOW_ME");
	step("sayteam_key");
	self pingPlayer();
	step("pingplayer");
	self.health = 0;
	self sayAll("zero health");
	step("sayall_zero_health");
	self sayTeam("zero health team");
	step("sayteam_zero_health");
	self.health = 100;
	self.sessionstate = "dead";
	self sayAll("dead state");
	step("sayall_dead_state");
	self.sessionstate = "spectator";
	self sayAll("spectator state");
	step("sayall_spectator_state");
	self.sessionstate = "playing";
	self.sessionteam = "spectator";
	self sayAll("spectator team");
	step("sayall_spectator_team");
	self sayTeam("spectator team only");
	step("sayteam_spectator_team");
	self.sessionteam = "none";
	self sayTeam("none team only");
	step("sayteam_none_team");
	logPrint("PROBE done\n");
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
