//	Whether `wait 0` yields to the other threads of the frame: two threads,
//	each logging, waiting 0 and logging again, started one after the other
//	at load, then two more the same way inside a later frame's pass.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level thread pair("a", 0);
	level thread pair("b", 0);
	level thread pair("c", 0.1);
	level thread pair("d", 0.1);
	wait 1;
	logPrint("PROBE wait0_yield done " + 1 + "\n");
}

pair(tag, delay)
{
	if (delay > 0)
		wait delay;
	logPrint("PROBE wait0_yield " + tag + " before\n");
	wait 0;
	logPrint("PROBE wait0_yield " + tag + " after\n");
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
