//	Writing `undefined` into an array: does it remove the key, so `.size`
//	drops? `_deathicons.gsc` (CoD2) shrinks its arrays with
//	`array[lastElement] = undefined` and walks them by `.size`.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	a[0] = "x";
	a[1] = "y";
	a[2] = "z";
	a[2] = undefined;
	logPrint("PROBE last_undef_size " + a.size + "\n");
	a[0] = undefined;
	logPrint("PROBE first_undef_size " + a.size + "\n");
	logPrint("PROBE first_undef_isdefined " + isdefined(a[0]) + "\n");
	logPrint("PROBE after_one " + a[1] + "\n");
	a[a.size] = "w";
	logPrint("PROBE append_size " + a.size + "\n");
	logPrint("PROBE append_at1 " + a[1] + "\n");

	level.undefarr[0] = "p";
	level.undefarr[1] = "q";
	level.undefarr[1] = undefined;
	logPrint("PROBE field_undef_size " + level.undefarr.size + "\n");

	b["k"] = 1;
	b["k"] = undefined;
	logPrint("PROBE strkey_undef_size " + b.size + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
