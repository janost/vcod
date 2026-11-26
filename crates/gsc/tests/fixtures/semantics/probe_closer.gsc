//	lengthSquared and closer (cod11-gsc-object-model.md 9.2). lengthSquared
//	sums the squares on the x87 stack and rounds once, so 4096^2 + 1 + 1
//	keeps both ones (2). closer(ref, a, b) is 1 when a's squared distance is
//	under b's; 1.1d stores the first distance as a float and keeps the
//	second and both differences on the stack, 1.5b and UO round each.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	logPrint("PROBE lsq_small " + lengthSquared((1, 2, 3)) + "\n");
	logPrint("PROBE lsq_frac " + lengthSquared((0.5, 0.25, 2)) + "\n");
	logPrint("PROBE lsq_one_round " + (lengthSquared((4096, 1, 1)) - 16777216) + "\n");
	logPrint("PROBE closer_near " + closer((0, 0, 0), (1, 0, 0), (2, 0, 0)) + "\n");
	logPrint("PROBE closer_far " + closer((0, 0, 0), (2, 0, 0), (1, 0, 0)) + "\n");
	logPrint("PROBE closer_tie " + closer((0, 0, 0), (1, 0, 0), (0, 1, 0)) + "\n");
	//	2^24 against 2^24 + 1: only an unrounded second distance tells them apart.
	logPrint("PROBE closer_second_unrounded " + closer((0, 0, 0), (4096, 0, 0), (4096, 1, 0)) + "\n");
	//	2^24 + 1 stored as a float is 2^24, under 2^24 + 0.25.
	logPrint("PROBE closer_first_rounds " + closer((0, 0, 0), (4096, 1, 0), (4096, 0.5, 0)) + "\n");
	//	2^24 + 1 as a float difference is 2^24: a's 2^48 is then under b's 2^48 + 2^25.
	logPrint("PROBE closer_diff_rounds " + closer((16777216, 0, 0), (-1, 0, 0), (33554432, 4096, 4096)) + "\n");
	logPrint("PROBE closer_done 1\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
