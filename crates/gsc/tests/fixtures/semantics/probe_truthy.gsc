//	Truthiness of a non-empty string and a zero vector.
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

	logPrint("PROBE at truthy_nonempty_string\n");
	if ("a")
		logPrint("PROBE truthy_nonempty_string 1\n");
	else
		logPrint("PROBE truthy_nonempty_string 0\n");
	logPrint("PROBE at truthy_zero_vector\n");
	v = (0, 0, 0);
	if (v)
		logPrint("PROBE truthy_zero_vector 1\n");
	else
		logPrint("PROBE truthy_zero_vector 0\n");
	logPrint("PROBE at truthy_undefined\n");
	if (isDefined(level.nosuchfield))
		logPrint("PROBE truthy_isdefined 1\n");
	else
		logPrint("PROBE truthy_isdefined 0\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
