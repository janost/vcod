//	Which main() runs first, the map's or the gametype's, and whether a
//	threaded function runs to its first wait before the caller continues.
//	mp_pavlov.gsc's own main() sets game["allies"] = "russian", so seeing it
//	here means the map script ran first.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at bootstrap_map_main_first\n");
	if (isDefined(game["allies"]))
		logPrint("PROBE bootstrap_game_allies " + game["allies"] + "\n");
	else
		logPrint("PROBE bootstrap_game_allies undefined\n");

	logPrint("PROBE at bootstrap_thread_runs_now\n");
	level.marker = "before";
	thread setMarker();
	logPrint("PROBE bootstrap_thread_ran_inline " + level.marker + "\n");
}

setMarker()
{
	level.marker = "after";
}

Callback_StartGameType()
{
	logPrint("PROBE at bootstrap_startgametype\n");
	if (isDefined(game["allies"]))
		logPrint("PROBE bootstrap_startgametype_allies " + game["allies"] + "\n");
	else
		logPrint("PROBE bootstrap_startgametype_allies undefined\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
