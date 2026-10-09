//	The level clock a gametype's first threads see (doc
//	cod11-map-cycle.md 3, step 20). No client needed. Logs getTime() at the
//	gametype's main(), at Callback_StartGameType, after the first waits a
//	thread started there issues, and the frame each 50 ms wait lands on.
//	Run: tools/run_probe.sh client-probes/probe_startclock mp_carentan

main()
{
	logPrint("PROBE main " + gettime() + "\n");
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType()
{
	logPrint("PROBE start " + gettime() + "\n");
	thread ticks();
	thread one();
	thread zero();
	thread tenth();
}

ticks()
{
	for (i = 0; i < 12; i++)
	{
		wait 0.05;
		logPrint("PROBE tick " + i + " " + gettime() + "\n");
	}
}

one()
{
	wait 1;
	logPrint("PROBE wait1 " + gettime() + "\n");
	wait 1;
	logPrint("PROBE wait1b " + gettime() + "\n");
}

zero()
{
	wait 0;
	logPrint("PROBE wait0 " + gettime() + "\n");
}

tenth()
{
	wait 0.1;
	logPrint("PROBE wait01 " + gettime() + "\n");
	wait 0.2;
	logPrint("PROBE wait02 " + gettime() + "\n");
}

Callback_PlayerConnect()
{
}

Callback_PlayerDisconnect()
{
}

Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
}

Callback_PlayerKilled(eInflictor, attacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc)
{
}
