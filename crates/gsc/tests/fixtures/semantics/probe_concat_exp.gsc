//	Where a float's `%g` rendering switches to the exponent form, and how
//	that form is spelled. mp_chateau's setExpFog(0.00001, ...) is the stock
//	case: its configstring goes out through the same formatter.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE exp_small " + 0.00001 + "\n");
	logPrint("PROBE exp_small_neg " + (0 - 0.00002) + "\n");
	logPrint("PROBE exp_small_digits " + 0.000012345678 + "\n");
	logPrint("PROBE exp_edge_low " + 0.0001 + "\n");
	logPrint("PROBE exp_edge_high " + 999999.0 + "\n");
	logPrint("PROBE exp_million " + 1000000.0 + "\n");
	logPrint("PROBE exp_big_digits " + 1234567.0 + "\n");
	logPrint("PROBE exp_huge " + (1000000.0 * 1000000.0 * 1000000.0 * 1000000.0 * 10000.0) + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
