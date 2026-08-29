//	`level` as the control for probe_game.gsc: dot read/write is expected
//	to work (13083 dot uses across the MP corpus, 0 bracket uses).
//	`level["k"] = "v"` is measured in probe_level_bracket.gsc, and
//	`level.size` in probe_level_size.gsc, both kept separate for their own
//	reasons noted in those files.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at level_dot_write\n");
	level.probeFoo = 1;
	logPrint("PROBE level_dot_read " + level.probeFoo + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
