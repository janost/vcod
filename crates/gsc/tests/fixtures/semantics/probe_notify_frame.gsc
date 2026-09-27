//	Which frame a notify's waiters resume on, and where in the notifier's own
//	run: a waiter started before the notifier and one started after it, and
//	each of them again past a `wait 0`. level.f is a frame counter a ticker
//	started first bumps once a frame, so equal numbers are the same frame.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level.f = 0;
	level thread ticker();
	level thread waiter("early");
	level thread waiter_wait0("early_wait0");
	level thread notifier();
	level thread waiter("late");
	level thread waiter_wait0("late_wait0");
	wait 1;
	logPrint("PROBE notify_frame done " + level.f + "\n");
}

ticker()
{
	for (;;)
	{
		wait 0.05;
		level.f = level.f + 1;
	}
}

notifier()
{
	wait 0.3;
	logPrint("PROBE notify_frame notify " + level.f + "\n");
	level notify("go");
	logPrint("PROBE notify_frame after_notify " + level.f + "\n");
}

waiter(tag)
{
	level waittill("go");
	logPrint("PROBE notify_frame resume " + tag + " " + level.f + "\n");
}

waiter_wait0(tag)
{
	level waittill("go");
	logPrint("PROBE notify_frame resume " + tag + " " + level.f + "\n");
	wait 0;
	logPrint("PROBE notify_frame past_wait0 " + tag + " " + level.f + "\n");
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
