//	Does `self` reach a callee that was called without a receiver?
//	`_callbacksetup.gsc` calls `[[level.callbackPlayerConnect]]()` with no
//	receiver and dm.gsc's `Callback_PlayerConnect` opens on `self.statusicon`,
//	so the whole client callback chain rests on this answer.
//	`level` is the receiver, not a spawned entity, so the measurement needs
//	no object table and runs in this crate's A/B unchanged.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	level.probe_mark = "marked";

	logPrint("PROBE at self_receiver\n");
	level withReceiver();
}

//	Called with `level` as the receiver; everything below inherits from here
//	or does not.
withReceiver()
{
	logPrint("PROBE self_receiver_defined " + isDefined(self) + "\n");

	logPrint("PROBE at self_plain_call\n");
	plainCallee();

	logPrint("PROBE at self_ptr_call\n");
	f = ::ptrCallee;
	[[f]]();

	logPrint("PROBE at self_threaded_call\n");
	thread threadedCallee();
}

plainCallee()
{
	logPrint("PROBE self_plain_defined " + isDefined(self) + "\n");
	if(isDefined(self))
		logPrint("PROBE self_plain_mark " + self.probe_mark + "\n");
}

ptrCallee()
{
	logPrint("PROBE self_ptr_defined " + isDefined(self) + "\n");
	if(isDefined(self))
		logPrint("PROBE self_ptr_mark " + self.probe_mark + "\n");
}

threadedCallee()
{
	logPrint("PROBE self_threaded_defined " + isDefined(self) + "\n");
	if(isDefined(self))
		logPrint("PROBE self_threaded_mark " + self.probe_mark + "\n");
}

Callback_StartGameType() {}
Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
