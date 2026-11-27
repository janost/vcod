//	`==` and `!=` on a NaN: a float product past the range is inf, and
//	inf - inf is NaN. Run by tools/run_probe.sh.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	big = 300000000000000000000000000000000000000.0;
	inf = big * 10;
	logPrint("PROBE inf_gt_big " + (inf > big) + "\n");
	nan = inf - inf;
	logPrint("PROBE nan_eq_nan " + (nan == nan) + "\n");
	logPrint("PROBE nan_ne_nan " + (nan != nan) + "\n");
	logPrint("PROBE nan_eq_zero " + (nan == 0) + "\n");
	logPrint("PROBE nan_ne_zero " + (nan != 0) + "\n");
	logPrint("PROBE nan_ne_one " + (nan != 1.5) + "\n");
	logPrint("PROBE nan_gt_zero " + (nan > 0) + "\n");
	logPrint("PROBE nan_lt_zero " + (nan < 0) + "\n");
	logPrint("PROBE inf_eq_inf " + (inf == inf) + "\n");
	logPrint("PROBE inf_ne_inf " + (inf != inf) + "\n");
	logPrint("PROBE done\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
