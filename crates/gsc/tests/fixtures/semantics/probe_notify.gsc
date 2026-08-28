//	Which of two waiters on one event wakes first.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.
//	Groups are separate files because a script runtime error takes the whole
//	retail server down, so one fatal expression would cost every measurement
//	after it.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level thread notify_waiter("first");
	level thread notify_waiter("second");
	wait 0.5;
	level notify("probe_event");
	wait 0.5;
	logPrint("PROBE notify_done " + 1 + "\n");
}

Callback_StartGameType() {}

notify_waiter(tag)
{
	level waittill("probe_event");
	logPrint("PROBE notify_order " + tag + "\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
