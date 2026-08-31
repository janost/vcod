//	What the engine has already put on a client entity by the time
//	Callback_PlayerConnect runs: is `.pers` an object script can index, is
//	`.name` filled in, and what does reading an index off a field that really
//	is undefined do.
//
//	NOT part of the A/B run. Every other probe is a gametype script that
//	measures from `main`, which tools/capture_probes.sh can capture because the
//	server needs no client to run it. This one measures from inside
//	Callback_PlayerConnect, so it needs a client to connect before it logs
//	anything at all; run_probe.sh boots the server alone and would capture an
//	empty section. Hence this directory rather than the one beside it, which
//	is what capture_probes.sh globs and what semantics_ab.rs pairs against
//	retail-captures.txt. See this directory's README.md for the run recipe and
//	the measurement it produced.
//
//	The last group is deliberately last: reading an index off an undefined
//	field is fatal on retail and takes the whole server down with it.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
	logPrint("PROBE at main\n");
}

Callback_StartGameType()
{
	logPrint("PROBE at startgametype\n");
}

Callback_PlayerConnect()
{
	logPrint("PROBE at connect_before_begin\n");
	if(isdefined(self.pers))
		logPrint("PROBE pers_before_begin defined\n");
	else
		logPrint("PROBE pers_before_begin undefined\n");

	self waittill("begin");

	logPrint("PROBE at connect_after_begin\n");
	if(isdefined(self.pers))
		logPrint("PROBE pers_after_begin defined\n");
	else
		logPrint("PROBE pers_after_begin undefined\n");

	logPrint("PROBE at read_pers_key\n");
	if(isdefined(self.pers["team"]))
		logPrint("PROBE pers_team defined\n");
	else
		logPrint("PROBE pers_team undefined\n");

	logPrint("PROBE at write_pers_key\n");
	self.pers["team"] = "spectator";
	logPrint("PROBE pers_team_written " + self.pers["team"] + "\n");

	logPrint("PROBE at read_name\n");
	logPrint("PROBE name " + self.name + "\n");

	logPrint("PROBE at read_undef_field_index\n");
	if(isdefined(self.nosuchfield["team"]))
		logPrint("PROBE undef_index defined\n");
	else
		logPrint("PROBE undef_index undefined\n");

	logPrint("PROBE at done\n");
}

Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
