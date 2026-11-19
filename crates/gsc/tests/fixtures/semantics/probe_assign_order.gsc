//	Which half of an index or field assignment runs first, and where a
//	write lands when the value expression changes the target. Run by
//	tools/run_probe.sh; every logPrint line is one measurement.

main()

{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level.log = "";
	a[side("k")] = side("v");
	logPrint("PROBE index_order " + level.log + "\n");

	level.arr[5] = "x";
	level.arr[0] = copy_then_write();
	logPrint("PROBE copied_target " + level.arr.size + " " + isdefined(level.arr[0]) + "\n");

	level.brr[5] = "x";
	level.brr[0] = clear_target();
	logPrint("PROBE cleared_target " + isdefined(level.brr) + "\n");

	level.o = spawnstruct();
	level.o.tag = "old";
	level.o.f = swap_object();
	logPrint("PROBE swapped_object " + level.o.tag + " " + isdefined(level.o.f) + "\n");

	c[0] = "x";
	c[1] = c;
	logPrint("PROBE self_store " + c.size + " " + c[1].size + "\n");
}


side(s)

{
	level.log = level.log + s;
	return s;
}


copy_then_write()

{
	b = level.arr;
	level.arr[1] = "y";
	return "z";
}


swap_object()

{
	level.o = spawnstruct();
	level.o.tag = "new";
	return "z";
}


clear_target()

{
	level.brr = undefined;
	return "z";
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
