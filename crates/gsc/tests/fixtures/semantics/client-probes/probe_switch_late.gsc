//	probe_switch_dev0 in a thread past a `wait`, for the builds where an
//	error in the load ends the level (CoD2, CoD4): at `developer 0` a float
//	matches no case and `default` runs. Run with PRE_MAP_ARGS='+set developer 0'
//	through tools/run_probe.sh (1.x) or tools/run_probe_cod4_gsc.sh; last line
//	`PROBE done`.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level thread late();
}

late()
{
	wait 0.5;
	logPrint("PROBE first_float " + sw_int(1.0) + "\n");
	logPrint("PROBE float_two " + sw_int(2.0) + "\n");
	logPrint("PROBE float_nodef " + sw_nodef(1.0) + "\n");
	logPrint("PROBE int_one " + sw_int(1) + "\n");
	logPrint("PROBE float_after_one " + sw_int(5.0) + "\n");
	logPrint("PROBE int_two " + sw_int(2) + "\n");
	logPrint("PROBE float_after_two " + sw_int(7.5) + "\n");
	logPrint("PROBE vec " + sw_int((1, 2, 3)) + "\n");
	logPrint("PROBE undef " + sw_int(level.unset) + "\n");
	logPrint("PROBE after_undef\n");
	logPrint("PROBE done\n");
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
