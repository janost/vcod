//	Are `true` and `false` language literals, and what do they evaluate to?
//	The stock corpus leans on them everywhere (`_gameobjects::main` sets
//	`dodelete = true` and later tests `if(dodelete)`), so the answer decides
//	whether those scripts run at all.
//	The case test goes last: if `TRUE` is not a literal it is an undefined
//	local, and concatenating one is fatal, which would cost every line after.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at bool_true_value\n");
	t = true;
	logPrint("PROBE bool_true_value " + t + "\n");

	logPrint("PROBE at bool_false_value\n");
	f = false;
	logPrint("PROBE bool_false_value " + f + "\n");

	logPrint("PROBE at bool_truthiness\n");
	if (t)
		logPrint("PROBE bool_true_is_truthy yes\n");
	else
		logPrint("PROBE bool_true_is_truthy no\n");
	if (f)
		logPrint("PROBE bool_false_is_truthy yes\n");
	else
		logPrint("PROBE bool_false_is_truthy no\n");

	logPrint("PROBE at bool_compares_to_int\n");
	if (true == 1)
		logPrint("PROBE bool_true_eq_1 yes\n");
	else
		logPrint("PROBE bool_true_eq_1 no\n");
	if (false == 0)
		logPrint("PROBE bool_false_eq_0 yes\n");
	else
		logPrint("PROBE bool_false_eq_0 no\n");

	logPrint("PROBE at bool_case\n");
	u = TRUE;
	logPrint("PROBE bool_upper_true_value " + u + "\n");
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
