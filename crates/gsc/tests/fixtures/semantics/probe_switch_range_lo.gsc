//	The bottom of an int `switch` key's range, probed key by key down from CoD1's -0x7e0000 past CoD2's -0x7e0002 to CoD4's -0x7e8000; each game stops at its own bound.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	wait 1;
	logPrint("PROBE at lo_cod1\n");
	logPrint("PROBE lo_cod1 " + sw(0 - 8257536) + "\n");
	logPrint("PROBE at lo_cod1_1\n");
	logPrint("PROBE lo_cod1_1 " + sw(0 - 8257537) + "\n");
	logPrint("PROBE at lo_cod2\n");
	logPrint("PROBE lo_cod2 " + sw(0 - 8257538) + "\n");
	logPrint("PROBE at lo_cod2_1\n");
	logPrint("PROBE lo_cod2_1 " + sw(0 - 8257539) + "\n");
	logPrint("PROBE at lo_cod4\n");
	logPrint("PROBE lo_cod4 " + sw(0 - 8290304) + "\n");
	logPrint("PROBE at lo_cod4_1\n");
	logPrint("PROBE lo_cod4_1 " + sw(0 - 8290305) + "\n");
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
