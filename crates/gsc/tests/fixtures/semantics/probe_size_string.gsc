//	`.size` on a string, on its own because it may be fatal.
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

	s = "abcd";
	logPrint("PROBE at size_string\n");
	logPrint("PROBE size_string " + s.size + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
