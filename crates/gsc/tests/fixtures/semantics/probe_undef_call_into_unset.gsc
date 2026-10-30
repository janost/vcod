//	`c[5] = nothing()` on a local that was never set, where `nothing()`
//	returns undefined: probe_undef_into_unset's literal `undefined` is
//	fatal; is an undefined value from a call too, or does it vivify the
//	array and drop the key? Run by tools/run_probe.sh.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at undef_call_into_unset\n");
	c[5] = nothing();
	logPrint("PROBE undef_call_isdefined " + isdefined(c) + "\n");
	logPrint("PROBE undef_call_size " + c.size + "\n");
}

nothing() {}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
