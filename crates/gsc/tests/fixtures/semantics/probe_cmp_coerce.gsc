//	Which way `==` coerces between a string and a number: numeric
//	reading of the string, or string reading of the number.
//	Run by tools/run_probe.sh.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at eq_str_dotzero_int\n");
	if ("5.0" == 5)
		logPrint("PROBE eq_str_dotzero_int 1\n");
	else
		logPrint("PROBE eq_str_dotzero_int 0\n");
	logPrint("PROBE at eq_str_padded_int\n");
	if ("05" == 5)
		logPrint("PROBE eq_str_padded_int 1\n");
	else
		logPrint("PROBE eq_str_padded_int 0\n");
	logPrint("PROBE at eq_str_word_zero\n");
	if ("abc" == 0)
		logPrint("PROBE eq_str_word_zero 1\n");
	else
		logPrint("PROBE eq_str_word_zero 0\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
