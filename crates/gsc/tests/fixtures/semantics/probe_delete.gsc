//	The deferred-free window. delete() in retail sets think = G_FreeEntity
//	and nextthink = level.time + 100, so the entity keeps its number and its
//	place in getEntArray for that window. Measured immediately after the
//	delete and again past 150 ms.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at delete_setup\n");
}

Callback_StartGameType()
{
	e1 = spawn("script_origin", (0, 0, 0));
	e2 = spawn("script_origin", (64, 0, 0));
	e1.targetname = "probe_del_a";
	e2.targetname = "probe_del_b";

	logPrint("PROBE at delete_before\n");
	before = getentarray("script_origin", "classname");
	logPrint("PROBE delete_count_before " + before.size + "\n");
	logPrint("PROBE delete_number_b " + e2 getEntityNumber() + "\n");

	logPrint("PROBE at delete_immediate\n");
	e1 delete();
	now = getentarray("script_origin", "classname");
	logPrint("PROBE delete_count_immediate " + now.size + "\n");

	logPrint("PROBE at delete_reuse_immediate\n");
	e3 = spawn("script_origin", (128, 0, 0));
	logPrint("PROBE delete_number_reuse_immediate " + e3 getEntityNumber() + "\n");

	logPrint("PROBE at delete_after_wait\n");
	wait 0.15;
	later = getentarray("script_origin", "classname");
	logPrint("PROBE delete_count_after_wait " + later.size + "\n");

	logPrint("PROBE at delete_reuse_after_wait\n");
	e3 delete();
	wait 0.15;
	e4 = spawn("script_origin", (192, 0, 0));
	logPrint("PROBE delete_number_reuse_after_wait " + e4 getEntityNumber() + "\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
