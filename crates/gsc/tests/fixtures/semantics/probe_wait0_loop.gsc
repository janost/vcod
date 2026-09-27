//	How many times a `wait 0` loop runs per server frame: a thousand turns of
//	`i++; wait 0;`, bounded because an unbounded one hangs a developer-1
//	server in the engine's infinite-loop warning. A logger started first reads
//	the count once a frame for five frames.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level.i = 0;
	level thread logger();
	level thread spinner();
	wait 1;
	logPrint("PROBE wait0_loop done " + level.i + "\n");
}

logger()
{
	for (n = 0; n < 5; n++)
	{
		wait 0.05;
		logPrint("PROBE wait0_loop frame " + n + " i " + level.i + "\n");
	}
}

spinner()
{
	wait 0.05;
	for (n = 0; n < 1000; n++)
	{
		level.i = level.i + 1;
		wait 0;
	}
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
