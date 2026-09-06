//	Whether a thread survives notifying an event it has itself registered an
//	`endon` for: stock `dm.gsc`'s `endMap` does exactly that, since it calls
//	`spawnIntermission` (which notifies "spawned") on every player from
//	inside a `Callback_PlayerKilled` thread that opened with
//	`self endon("spawned")`, and then waits ten seconds before `exitLevel`.
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

	level thread self_notifier();
	wait 3;
	logPrint("PROBE endon_self done " + 1 + "\n");
}

Callback_StartGameType() {}

self_notifier()
{
	level endon("probe_event");
	logPrint("PROBE endon_self armed\n");
	level notify("probe_event");
	logPrint("PROBE endon_self after_notify\n");
	wait 1;
	logPrint("PROBE endon_self after_wait\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
