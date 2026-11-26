//	vectorDot's sum: 1.1d 0x5e9d4 adds the three products on the x87 stack
//	and rounds once, so 2^24 + 1 + 1 keeps both ones (2), where a float
//	running sum drops each (0).
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	logPrint("PROBE dot_small " + vectorDot((1, 2, 3), (4, -5, 6)) + "\n");
	logPrint("PROBE dot_frac " + vectorDot((0.5, 0.25, 2), (3, 8, 0.125)) + "\n");
	logPrint("PROBE dot_one_round " + (vectorDot((16777216, 1, 1), (1, 1, 1)) - 16777216) + "\n");
	logPrint("PROBE dot_done 1\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
