//	The teamkill complaint `allowComplaint` opens and `vote` answers, for
//	docs/research/cod11-votes.md section 3. Two plain `--net-probe` clients,
//	both axis: slot 0 is the offender, slot 1 the victim, whose window on
//	slot 0 the script opens every five seconds; see this directory's
//	README.md, "probe_complaint". `probe_players 3` adds a second victim,
//	slot 2, whose window opens beside slot 1's.

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
	level.players = [];
}

Callback_PlayerConnect()
{
	self waittill("begin");
	n = self getEntityNumber();
	self.sessionteam = "axis";
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	sp = spawns[n];
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(sp.origin, sp.angles);
	logPrint("PROBE begin " + n + "\n");
	level.players[n] = self;
	level.joined++;
	if(n >= 1)
		self thread complain();
}

complain()
{
	want = getCvarInt("probe_players");
	if(want < 2)
		want = 2;
	while(level.joined < want)
		wait 0.5;
	wait 2;
	self allowComplaint(self);
	logPrint("PROBE self\n");
	wait 1;
	for(i = 0; i < 6; i++)
	{
		if(isDefined(level.players[0]))
		{
			self allowComplaint(level.players[0]);
			logPrint("PROBE open " + i + " at " + getTime() + "\n");
		}
		wait 5;
	}
	logPrint("PROBE done\n");
}

Callback_PlayerDisconnect()
{
	n = self getEntityNumber();
	level.players[n] = undefined;
	logPrint("PROBE disconnect " + n + " at " + getTime() + "\n");
}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
