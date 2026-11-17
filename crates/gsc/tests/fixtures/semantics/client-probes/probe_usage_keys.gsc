//	`scriptUsage`'s entity line when the compiled scripts name three radiant
//	keys, against `probe_usage_ents` at `probe_mode base`, which names none
//	(docs/research/cod11-server-handshake.md, "Console commands over rcon").
//	`keys` never runs: naming the keys is the whole probe.

main()
{
	level.callbackStartGameType = ::blank;
	level.callbackPlayerConnect = ::blank;
	level.callbackPlayerDisconnect = ::blank;
	level.callbackPlayerDamage = ::blank;
	level.callbackPlayerKilled = ::blank;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
	thread run();
}

blank()
{
}

run()
{
	wait 1;
	logPrint("PROBE ready\n");
}

keys(e)
{
	x = e.export;
	x = e.script_noteworthy;
	x = e.script_gameobjectname;
}
