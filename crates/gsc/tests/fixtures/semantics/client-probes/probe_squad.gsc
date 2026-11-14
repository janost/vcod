//	1.5's squad say between squad members (cod15-protocol-diff.md 13).
//	Plain --net-probe clients, each sending its say with --probe-say; the
//	script puts every client on a side and in a squad by entity number at
//	its begin, so no stock menu runs:
//	0 axis squad_alpha, 1 axis squad_alpha, 2 axis squad_bravo,
//	3 allies squad_alpha. No stock 1.5 script sets `sessionsquad`.
//
//	PROBE squad <entity number> <team> <squad>

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
	self waittill("begin");
	n = self getEntityNumber();
	spot = getentarray("mp_deathmatch_spawn", "classname")[n];
	team = "axis";
	if(n == 3)
		team = "allies";
	squad = "squad_alpha";
	if(n == 2)
		squad = "squad_bravo";
	self.sessionteam = team;
	self.sessionstate = "playing";
	self.maxhealth = 100;
	self.health = 100;
	self spawn(spot.origin, spot.angles);
	self.sessionsquad = squad;
	logPrint("PROBE squad " + n + " " + self.sessionteam + " " + self.sessionsquad + "\n");
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
