//	Whether script reaches TR_SINE (movers doc 12). No client needed. Only
//	SP_func_bobbing (game.mp 0x583d0) and SP_func_pendulum (0x56fe0) write
//	trType 4, and spawn(classname, origin) reaches them through
//	G_CallSpawnEntity's spawns table. Logs both spawns, then the bobbing's
//	origin and the pendulum's angles every frame until the server stops.
//	The console log's tail holds how it stopped.
//	Run: tools/run_probe.sh client-probes/probe_sine mp_carentan

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType()
{
	thread run();
}

run()
{
	wait 1;
	b = spawn("func_bobbing", (0, 0, 100));
	logPrint("PROBE bobbing " + gettime() + " " + b getEntityNumber() + " " + b.classname + " model '" + b.model + "'\n");
	p = spawn("func_pendulum", (0, 0, 100));
	logPrint("PROBE pendulum " + gettime() + " " + p getEntityNumber() + " " + p.classname + "\n");
	for (i = 0; i < 100; i++)
	{
		wait 0.05;
		logPrint("PROBE f " + gettime() + " " + b.origin + " " + p.angles + "\n");
	}
	logPrint("PROBE done\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
