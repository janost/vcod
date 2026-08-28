//	Cross-type comparison. Checkpointed: if the run dies, the last `at` names the killer.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.
//	Groups are separate files because a script runtime error takes the whole
//	retail server down, so one fatal expression would cost every measurement
//	after it.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at eq_int_float\n");
	if (1 == 1.0)
		logPrint("PROBE eq_int_float 1\n");
	else
		logPrint("PROBE eq_int_float 0\n");
	logPrint("PROBE at eq_string_case\n");
	if ("ABC" == "abc")
		logPrint("PROBE eq_string_case 1\n");
	else
		logPrint("PROBE eq_string_case 0\n");
	logPrint("PROBE at eq_string_same\n");
	if ("abc" == "abc")
		logPrint("PROBE eq_string_same 1\n");
	else
		logPrint("PROBE eq_string_same 0\n");
	logPrint("PROBE at eq_undefined_int\n");
	if (undefined == 0)
		logPrint("PROBE eq_undefined_int 1\n");
	else
		logPrint("PROBE eq_undefined_int 0\n");
	logPrint("PROBE at eq_string_int\n");
	if ("5" == 5)
		logPrint("PROBE eq_string_int 1\n");
	else
		logPrint("PROBE eq_string_int 0\n");
	logPrint("PROBE at lt_string_string\n");
	if ("a" < "b")
		logPrint("PROBE lt_string_string 1\n");
	else
		logPrint("PROBE lt_string_string 0\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
