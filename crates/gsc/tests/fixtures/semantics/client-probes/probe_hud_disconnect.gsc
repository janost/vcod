//	What a disconnect does to the HUD elements the leaving client owns:
//	`ClientDisconnect` calls `HudElem_ClientDisconnect` ahead of
//	`Scr_PlayerDisconnect`, so a `newClientHudElem` record should already be
//	freed (isdefined 0) when the callback runs, while a `newHudElem` one is
//	left alone. Needs one client to connect, then leave; see this
//	directory's README.md for the recipe and the measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType() {}

Callback_PlayerConnect()
{
	self waittill("begin");
	level.own = newClientHudElem(self);
	level.shared = newHudElem();
	logPrint("PROBE connect " + self getEntityNumber() + " own " + isdefined(level.own) + " shared " + isdefined(level.shared) + "\n");
}

Callback_PlayerDisconnect()
{
	logPrint("PROBE disconnect_callback own " + isdefined(level.own) + " shared " + isdefined(level.shared) + "\n");
	level thread after_disconnect();
}

after_disconnect()
{
	wait 0.05;
	logPrint("PROBE next_frame own " + isdefined(level.own) + " shared " + isdefined(level.shared) + "\n");
}

Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
