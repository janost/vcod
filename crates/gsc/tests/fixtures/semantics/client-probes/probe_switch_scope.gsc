//	Whose last switch key 1.5b's float switch reads: the thread's own. After
//	`switch (2)` in `main` a float runs `case 2:` there, while a `thread` or
//	a thread resumed after a `wait` runs `default`. Run at `developer 0` on
//	1.5b (COD_VERSION=1.5 tools/run_probe.sh client-probes/probe_switch_scope);
//	last line `PROBE done`.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE main_two " + sw_int(2) + "\n");
	level thread b();
}

b()
{
	wait 0.5;
	logPrint("PROBE b_float " + sw_int(9.5) + "\n");
	logPrint("PROBE b_one " + sw_int(1) + "\n");
	level thread c();
	wait 0.5;
	logPrint("PROBE b_float_after_c " + sw_int(9.5) + "\n");
	logPrint("PROBE done\n");
}

c()
{
	logPrint("PROBE c_float " + sw_int(9.5) + "\n");
	logPrint("PROBE c_two " + sw_int(2) + "\n");
}

sw_int(v)
{
	r = "none";
	switch (v)
	{
	case 1:
		r = "one";
		break;
	case 2:
		r = "two";
		break;
	default:
		r = "default";
		break;
	}
	return r;
}

sw_nodef(v)
{
	r = "none";
	switch (v)
	{
	case 1:
		r = "one";
		break;
	}
	return r;
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
