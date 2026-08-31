//	Unary ! applied to a non-empty string and to a cvar read. probe_truthy
//	records if ("a") as fatal (cannot cast "a" to bool), but
//	_teams::restrictPlacedWeapons runs if(!getCvar("scr_allow_m1carbine"))
//	on every stock dm server and the bootstrap completes, so ! and if
//	differ. The empty-string case is the fatal one and lives alone in
//	probe_not_empty_string.gsc so it does not cost the getCvar measurement
//	here, which decides whether stock placed fg42 weapons get deleted.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at not_string_one\n");
	if (!"1")
		logPrint("PROBE not_string_one 1\n");
	else
		logPrint("PROBE not_string_one 0\n");

	logPrint("PROBE at not_string_zero\n");
	if (!"0")
		logPrint("PROBE not_string_zero 1\n");
	else
		logPrint("PROBE not_string_zero 0\n");

	logPrint("PROBE at not_getcvar_allow\n");
	if (!getCvar("scr_allow_fg42"))
		logPrint("PROBE not_getcvar_allow_fg42 1\n");
	else
		logPrint("PROBE not_getcvar_allow_fg42 0\n");
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
