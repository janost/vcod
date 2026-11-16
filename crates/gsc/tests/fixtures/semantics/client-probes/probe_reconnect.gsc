//	A client that connects again into the slot it still holds
//	(`SV_DirectConnect`'s reconnect arm), and the leaver's own lines: whether
//	the game hears the old client go, whether a thread on the old player
//	outlives it, and whether a command the disconnect callback sends the
//	leaver alone reaches its zombie. See this directory's README.md,
//	"probe_reconnect".

main()

{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}


Callback_StartGameType()

{
	level.gen = 0;
}

Callback_PlayerConnect()

{
	gen = level.gen;
	level.gen++;
	num = self getEntityNumber();
	logPrint("PROBE connect " + num + " gen " + gen + " field " + isDefined(self.probe_gen) + " pers " + isDefined(self.pers["probe"]) + " t " + getTime() + "\n");
	self.probe_gen = gen;
	self.pers["probe"] = gen;
	self thread alive(num, gen);
	self waittill("begin");
	hud = newClientHudElem(self);
	hud.x = 100 + gen;
	hud.y = 200;
	hud setValue(gen);
	logPrint("PROBE begin " + num + " gen " + gen + " t " + getTime() + "\n");
}


//	No endon: a drop frees the entity and kills it; only a player the game
//	never let go of keeps it running.
alive(num, gen)

{
	for(;;)
	{
		wait 1;
		logPrint("PROBE alive " + num + " gen " + gen + " now " + self.probe_gen + " t " + getTime() + "\n");
	}
}


Callback_PlayerDisconnect()

{
	num = self getEntityNumber();
	logPrint("PROBE disconnect_callback " + num + " gen " + self.probe_gen + " t " + getTime() + "\n");
	self iPrintLn("PROBE_SELF " + num);
	self setClientCvar("probe_self", num);
	iPrintLn("PROBE_ALL " + num);
}


Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
