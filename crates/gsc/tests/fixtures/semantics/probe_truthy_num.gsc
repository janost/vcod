//	Truthiness of numbers, the baseline the corpus does rely on.
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

	logPrint("PROBE at truthy_int_zero\n");
	if (0)
		logPrint("PROBE truthy_int_zero 1\n");
	else
		logPrint("PROBE truthy_int_zero 0\n");
	logPrint("PROBE at truthy_int_one\n");
	if (1)
		logPrint("PROBE truthy_int_one 1\n");
	else
		logPrint("PROBE truthy_int_one 0\n");
	logPrint("PROBE at truthy_float_zero\n");
	if (0.0)
		logPrint("PROBE truthy_float_zero 1\n");
	else
		logPrint("PROBE truthy_float_zero 0\n");
	logPrint("PROBE at truthy_float_half\n");
	if (0.5)
		logPrint("PROBE truthy_float_half 1\n");
	else
		logPrint("PROBE truthy_float_half 0\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
