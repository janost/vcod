//	Isolated: `game.foo = 1`, the construct that decides whether `game` is
//	strictly array-typed on retail. Alone in its own file because retail
//	rejected it as a *compile*-time "not an object" error, which voids the
//	whole script -- unlike a runtime error, nothing before it in the same
//	file would even get to run.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at game_dot_write\n");
	game.foo = 1;
	logPrint("PROBE game_dot_write_ok 1\n");
	logPrint("PROBE game_dot_read " + game.foo + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
