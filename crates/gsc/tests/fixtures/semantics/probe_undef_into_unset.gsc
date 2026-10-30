//	Writing `undefined` into an index of a local that was never set:
//	`c[5] = 1` there auto-vivifies an array (probe_autoviv); does
//	`c[5] = undefined` too, or is it fatal? Alone in its file since it may
//	kill the server. Run by tools/run_probe.sh; every logPrint line is one
//	measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	b[5] = 1;
	logPrint("PROBE int_into_unset_size " + b.size + "\n");
	logPrint("PROBE at undef_into_unset\n");
	c[5] = undefined;
	logPrint("PROBE undef_into_unset_isdefined " + isdefined(c) + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
