//	getentarray's return order, measured from StartGameType so the map's own entities exist.
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

	logPrint("PROBE at ents_deferred\n");
}

Callback_StartGameType()
{
	e1 = spawn("script_origin", (0, 0, 0));
	e2 = spawn("script_origin", (64, 0, 0));
	e3 = spawn("script_origin", (128, 0, 0));
	e1.targetname = "probe_a";
	e2.targetname = "probe_b";
	e3.targetname = "probe_c";

	ents = getentarray("script_origin", "classname");
	logPrint("PROBE entarray_count " + ents.size + "\n");
	for (i = 0; i < ents.size; i++)
		logPrint("PROBE entarray_order " + i + " " + ents[i].targetname + "\n");

	logPrint("PROBE at ents_numbers\n");
	for (i = 0; i < ents.size; i++)
	{
		n = ents[i] getEntityNumber();
		logPrint("PROBE entarray_number " + i + " " + n + "\n");
	}
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
