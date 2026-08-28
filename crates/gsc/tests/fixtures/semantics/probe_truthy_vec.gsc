//	Truthiness of a vector.
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

	v = (0, 0, 0);
	logPrint("PROBE at truthy_zero_vector\n");
	if (v)
		logPrint("PROBE truthy_zero_vector 1\n");
	else
		logPrint("PROBE truthy_zero_vector 0\n");
	w = (1, 0, 0);
	logPrint("PROBE at truthy_nonzero_vector\n");
	if (w)
		logPrint("PROBE truthy_nonzero_vector 1\n");
	else
		logPrint("PROBE truthy_nonzero_vector 0\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
