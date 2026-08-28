//	How a number renders when concatenated onto a string.
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

	logPrint("PROBE concat_int " + 5 + "\n");
	logPrint("PROBE concat_negative_int " + (0 - 5) + "\n");
	logPrint("PROBE concat_float_half " + 0.5 + "\n");
	logPrint("PROBE concat_float_whole " + 2.0 + "\n");
	logPrint("PROBE concat_float_long " + 0.8 + "\n");
	logPrint("PROBE concat_float_third " + (1.0 / 3) + "\n");
	logPrint("PROBE concat_big " + 1000000 + "\n");
	logPrint("PROBE at concat_undefined\n");
	logPrint("PROBE concat_undefined " + undefined + "\n");
	logPrint("PROBE at concat_vector\n");
	logPrint("PROBE concat_vector " + (1, 2, 3) + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
