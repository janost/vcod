//	grenadeExplosionEffect's two entities (combat doc 13.4). No client
//	needed. The builtin (game.mp 0x5aea4) raises an EV_GRENADE_EXPLODE temp
//	entity and then Concussive_fx's entity, each off G_Spawn; the spawns
//	after it, one a frame, show which numbers the call took and on which
//	frame each returns to the free list.
//	Run: tools/run_probe.sh client-probes/probe_concnum mp_carentan

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

num(tag, e)
{
	logPrint("PROBE " + tag + " " + gettime() + " " + e getEntityNumber() + "\n");
}

run()
{
	wait 1;
	a = spawn("script_origin", (0, 0, 0));
	num("a", a);
	grenadeExplosionEffect((0, 0, 0));
	b = spawn("script_origin", (0, 0, 0));
	num("b", b);
	keep = [];
	for (i = 1; i <= 18; i++)
	{
		wait 0.05;
		keep[i] = spawn("script_origin", (0, 0, 0));
		num("s_" + (i * 50), keep[i]);
	}
	logPrint("PROBE done\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
