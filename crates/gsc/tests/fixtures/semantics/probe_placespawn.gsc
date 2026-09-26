//	Where placeSpawnpoint puts every mp_deathmatch_spawn of the map it runs on.
//	Run by tools/run_probe.sh; one logPrint line per spawnpoint. Its
//	retail-captures.txt section is mp_pavlov's; the other stock maps, taken with
//	`tools/run_probe.sh probe_placespawn <map>`, are
//	crates/server/tests/fixtures/spawnpoints/placespawn-retail.txt.

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
	spawnpoints = getentarray("mp_deathmatch_spawn", "classname");
	for (i = 0; i < spawnpoints.size; i++)
	{
		before = spawnpoints[i].origin;
		spawnpoints[i] placeSpawnpoint();
		logPrint("PROBE place " + spawnpoints[i] getEntityNumber() + " " + before + " " + spawnpoints[i].origin + "\n");
	}
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
