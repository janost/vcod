//	Isolated: `level.size` after one and two dot-set fields. It turned out
//	to read 1 on retail regardless of how many fields `level` carries (not
//	the array-style key count `.size` gives an array or the character count
//	it gives a string), a struct `.size` semantic this task's scope did not
//	call for fixing -- see semantics_ab.rs's KNOWN_GAPS_OUT_OF_SCOPE
//	sibling comment and docs/research/cod11-gsc-language.md section 9.
//	Alone in its own file so this open question does not cost
//	probe_level.gsc's dot write/read measurement, which already matches.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level.probeFoo = 1;
	logPrint("PROBE level_size_after_one_field " + level.size + "\n");
	level.probeBar = 2;
	logPrint("PROBE level_size_after_two_fields " + level.size + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
