//	Numeric promotion across the operators.
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

	logPrint("PROBE div_int_int " + (1 / 2) + "\n");
	logPrint("PROBE div_int_int_exact " + (4 / 2) + "\n");
	logPrint("PROBE div_mixed " + (3 / 2.0) + "\n");
	logPrint("PROBE mul_int_int " + (3 * 3) + "\n");
	logPrint("PROBE mul_mixed " + (3 * 1.5) + "\n");
	logPrint("PROBE add_int_int " + (1 + 2) + "\n");
	logPrint("PROBE add_mixed " + (1 + 0.5) + "\n");
	logPrint("PROBE mod_int_int " + (7 % 3) + "\n");
	logPrint("PROBE mod_negative " + (0 - 7) % 3 + "\n");
	logPrint("PROBE neg_int " + (0 - 5) + "\n");
	logPrint("PROBE at div_by_zero\n");
	logPrint("PROBE div_by_zero " + (1 / 0) + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
