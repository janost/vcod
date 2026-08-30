//	Cvar coercion and the small level-state builtins dm.gsc's main() calls.
//	setCvar/getCvar round trip, atoi/atof semantics on a non-numeric string,
//	name case sensitivity, getTime's units and randomInt's bounds.
//	exitLevel is deliberately absent: it would end the map mid-probe.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at cvar_roundtrip\n");
	setCvar("probe_scratch", "hello");
	logPrint("PROBE cvar_roundtrip " + getCvar("probe_scratch") + "\n");

	logPrint("PROBE at cvar_unset\n");
	logPrint("PROBE cvar_unset [" + getCvar("probe_no_such_cvar") + "]\n");

	logPrint("PROBE at cvar_case\n");
	setCvar("probe_MixedCase", "7");
	logPrint("PROBE cvar_case_same [" + getCvar("probe_MixedCase") + "]\n");
	logPrint("PROBE cvar_case_lower [" + getCvar("probe_mixedcase") + "]\n");

	logPrint("PROBE at cvar_int_of_empty\n");
	logPrint("PROBE cvar_int_of_empty " + getCvarInt("probe_no_such_cvar") + "\n");

	logPrint("PROBE at cvar_int_of_text\n");
	setCvar("probe_text", "abc");
	logPrint("PROBE cvar_int_of_text " + getCvarInt("probe_text") + "\n");
	logPrint("PROBE cvar_float_of_text " + getCvarFloat("probe_text") + "\n");

	logPrint("PROBE at cvar_int_of_trailing\n");
	setCvar("probe_trailing", "12abc");
	logPrint("PROBE cvar_int_of_trailing " + getCvarInt("probe_trailing") + "\n");

	logPrint("PROBE at cvar_float_precision\n");
	setCvar("probe_third", "0.3333333333");
	logPrint("PROBE cvar_float_precision " + getCvarFloat("probe_third") + "\n");

	logPrint("PROBE at cvar_gettime\n");
	logPrint("PROBE cvar_gettime_positive " + (getTime() >= 0) + "\n");

	logPrint("PROBE at cvar_randomint\n");
	r = randomInt(1);
	logPrint("PROBE cvar_randomint_of_one " + r + "\n");
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
