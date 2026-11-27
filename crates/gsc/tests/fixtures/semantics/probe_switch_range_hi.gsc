//	The top of an int `switch` key's range, 0x7fffff. Past a `wait`, where developer 0 runs on. Run by tools/run_probe.sh; the key past it is last.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	wait 1;
	logPrint("PROBE hi_edge " + sw(8388607) + "\n");
	logPrint("PROBE at hi\n");
	logPrint("PROBE hi " + sw(8388608) + "\n");
	logPrint("PROBE done\n");
}

sw(v)
{
	r = "none";
	switch (v)
	{
	case 1:
		r = "one";
		break;
	default:
		r = "default";
		break;
	}
	return r;
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
