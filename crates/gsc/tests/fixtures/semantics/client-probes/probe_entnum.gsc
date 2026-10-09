//	Entity numbers and the entity pass's place in G_RunFrame (combat doc
//	14.7, "Entity numbers"). No client needed. A playFx temp entity takes
//	a number from G_Spawn and frees it on a later frame's entity pass;
//	the spawns around it show which number and on which frame it returns
//	to the free list. A deleted entity's G_FreeEntity think comes due 100 ms
//	on; the reads 50, 100 and 150 ms after the delete show whether that
//	frame's woken waits run before the think or after it. A mover's
//	movedone shows the frame the notify reaches a waiter on.
//	Run: tools/run_probe.sh client-probes/probe_entnum mp_carentan

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
	level.fx = loadfx("fx/explosions/grenade1.efx");
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
	playFx(level.fx, (0, 0, 0));
	b = spawn("script_origin", (0, 0, 0));
	num("b", b);
	wait 0.3;
	c = spawn("script_origin", (0, 0, 0));
	num("c_300", c);
	wait 0.05;
	d = spawn("script_origin", (0, 0, 0));
	num("d_350", d);
	wait 0.05;
	e = spawn("script_origin", (0, 0, 0));
	num("e_400", e);
	wait 0.05;
	f = spawn("script_origin", (0, 0, 0));
	num("f_450", f);

	wait 1;
	x = spawn("script_origin", (0, 0, 0));
	num("x", x);
	x delete();
	logPrint("PROBE del " + gettime() + " " + isdefined(x) + "\n");
	for (i = 1; i <= 3; i++)
	{
		wait 0.05;
		logPrint("PROBE del_" + (i * 50) + " " + gettime() + " " + isdefined(x) + "\n");
	}

	wait 1;
	m = spawn("script_model", (0, 0, 0));
	t0 = gettime();
	m moveto((0, 0, 100), 0.1);
	m waittill("movedone");
	logPrint("PROBE movedone " + t0 + " " + gettime() + "\n");
	logPrint("PROBE done\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
