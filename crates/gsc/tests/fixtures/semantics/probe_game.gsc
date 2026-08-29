//	Whether `game` is array-typed: bracket read/write, `.size` growth, and
//	whether it survives a `wait` (the cross-map persistent global in
//	retail). `game.foo = 1` is measured in probe_game_dotwrite.gsc instead
//	of here: it turned out to be a *compile*-time error on retail ("not an
//	object"), which voids the whole script before main() ever runs, so it
//	cannot share a file with anything meant to execute.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at game_bracket_write\n");
	game["allies"] = "russian";
	logPrint("PROBE game_bracket_read " + game["allies"] + "\n");
	logPrint("PROBE game_size_after_one " + game.size + "\n");
	game["axis"] = "german";
	logPrint("PROBE game_size_after_two " + game.size + "\n");

	logPrint("PROBE at game_persist\n");
	wait 0.05;
	logPrint("PROBE game_persist_after_wait " + game["allies"] + "\n");
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
