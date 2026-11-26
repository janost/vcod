//	`==` and `!=` on two numbers, around retail's 1e-6 tolerance.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE eq_tiny_zero " + (0.00000000000000000001 == 0) + "\n");
	logPrint("PROBE gt_tiny_zero " + (0.00000000000000000001 > 0) + "\n");
	logPrint("PROBE eq_thresh_zero " + (0.000001 == 0) + "\n");
	logPrint("PROBE eq_zero_thresh " + (0 == 0.000001) + "\n");
	logPrint("PROBE eq_below_zero " + (0.00000099999994 == 0) + "\n");
	logPrint("PROBE eq_neg_below_zero " + ((0 - 0.00000099999994) == 0) + "\n");
	logPrint("PROBE eq_neg_thresh_zero " + ((0 - 0.000001) == 0) + "\n");
	logPrint("PROBE eq_above_zero " + (0.0000010000001 == 0) + "\n");
	logPrint("PROBE eq_thresh_tiny " + (0.000001 == 0.00000000000000000001) + "\n");
	logPrint("PROBE eq_thresh_e24 " + (0.000001 == 0.000000000000000000000001) + "\n");
	logPrint("PROBE eq_thresh_e30 " + (0.000001 == 0.000000000000000000000000000001) + "\n");
	logPrint("PROBE eq_thresh_tie " + (0.000001 == 0.0000000000000000000000000516987882845642297) + "\n");
	logPrint("PROBE eq_thresh_past_tie " + (0.000001 == 0.0000000000000000000000000775481824268463445) + "\n");
	logPrint("PROBE ne_thresh_tiny " + (0.000001 != 0.00000000000000000001) + "\n");
	logPrint("PROBE eq_tiny_thresh " + (0.00000000000000000001 == 0.000001) + "\n");
	logPrint("PROBE eq_int_ulp " + (1 == 1.0000001) + "\n");
	logPrint("PROBE ne_int_ulp " + (1 != 1.0000001) + "\n");
	logPrint("PROBE eq_int_near " + (1 == 1.000001) + "\n");
	logPrint("PROBE eq_int_far " + (1 == 1.00001) + "\n");
	logPrint("PROBE eq_neg_ulp " + ((0 - 1) == (0 - 1.0000001)) + "\n");
	logPrint("PROBE eq_large_float " + (100000.0 == 100000.01) + "\n");
	logPrint("PROBE eq_large_int_float " + (16777217 == 16777216.0) + "\n");
	logPrint("PROBE eq_large_int_int " + (16777217 == 16777216) + "\n");
	logPrint("PROBE ne_large_int_int " + (16777217 != 16777216) + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
