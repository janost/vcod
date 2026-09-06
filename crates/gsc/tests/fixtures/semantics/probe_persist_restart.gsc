//	What survives map_restart(true): game[], level and a spawned entity.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.
//	map_restart(true) reloads the same map in place and runs main() again, so
//	this file's main() prints the before half on its first pass and the after
//	half on its second.
//	A cvar, not game[], is what tells the two apart: game[] is exactly what is
//	under test, and a guard that did not survive would put the server in a
//	reload loop.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	if (getCvar("probe_persist_pass") != "")
	{
		logPrint("PROBE after_pass " + getCvar("probe_persist_pass") + "\n");
		logPrint("PROBE after_game_str_defined " + isdefined(game["str"]) + "\n");
		logPrint("PROBE after_game_num_defined " + isdefined(game["num"]) + "\n");
		logPrint("PROBE after_game_ent_defined " + isdefined(game["ent"]) + "\n");
		logPrint("PROBE after_level_y_defined " + isdefined(level.y) + "\n");
		if (isdefined(game["str"]))
			logPrint("PROBE after_game_str " + game["str"] + "\n");
		if (isdefined(game["num"]))
			logPrint("PROBE after_game_num " + game["num"] + "\n");
		return;
	}

	setCvar("probe_persist_pass", "2");
	game["str"] = "kept";
	game["num"] = 7;
	level.y = 7;
	logPrint("PROBE before_game_str " + game["str"] + "\n");
	logPrint("PROBE before_game_num " + game["num"] + "\n");
	logPrint("PROBE before_level_y " + level.y + "\n");
	logPrint("PROBE at spawn\n");
	game["ent"] = spawn("script_model", (0, 0, 0));
	logPrint("PROBE before_game_ent_defined " + isdefined(game["ent"]) + "\n");
	thread end_level_soon();
}

end_level_soon()
{
	wait 2;
	logPrint("PROBE at end_level\n");
	map_restart(true);
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
