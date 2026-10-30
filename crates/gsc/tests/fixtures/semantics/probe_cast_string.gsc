//	What `(int)` and `(float)` make of a string: UO's stock `_birdbush.gsc`
//	adds `(int) self.script_noteworthy`, a BSP key, to `(int) gettime()`.
//	The non-numeric string goes last: a failed cast is fatal.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at cast_int_num\n");
	logPrint("PROBE cast_int_num " + ((int) "5000" + 1) + "\n");
	logPrint("PROBE at cast_int_space\n");
	logPrint("PROBE cast_int_space " + ((int) " -7" + 1) + "\n");
	logPrint("PROBE at cast_int_tail\n");
	logPrint("PROBE cast_int_tail " + ((int) "12abc" + 1) + "\n");
	logPrint("PROBE at cast_int_frac\n");
	logPrint("PROBE cast_int_frac " + ((int) "3.9" + 1) + "\n");
	logPrint("PROBE at cast_int_zerofrac\n");
	logPrint("PROBE cast_int_zerofrac " + ((int) "0.9" + 1) + "\n");
	logPrint("PROBE at cast_int_dotzero\n");
	logPrint("PROBE cast_int_dotzero " + ((int) ".05" + 1) + "\n");
	logPrint("PROBE at cast_float_num\n");
	logPrint("PROBE cast_float_num " + ((float) "2.5" + 1) + "\n");
	logPrint("PROBE at cast_float_tail\n");
	logPrint("PROBE cast_float_tail " + ((float) "1.5e1x" + 1) + "\n");
	logPrint("PROBE at cast_int_word\n");
	logPrint("PROBE cast_int_word " + ((int) "abc" + 1) + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
