// addTestClient across a map_restart (docs/research/cod11-gsc-object-model.md
// 25): the first level adds a test client past sv_reconnectlimit and calls
// map_restart(false); the second logs what the connect callback saw and
// which players the level holds, then adds another.

main()
{
	maps\mp\gametypes\dm::main();
	level.callbackPlayerConnect = ::onconnect;
	thread run();
}

run()
{
	pass = getcvar("probe_tc_pass");
	logPrint("PROBE level " + pass + " t " + getTime() + "\n");
	if (pass == "")
	{
		wait 5;
		ent = addtestclient();
		logPrint("PROBE added num " + ent getEntityNumber() + " name " + ent.name + "\n");
		wait 1;
		setcvar("probe_tc_pass", "1");
		logPrint("PROBE restart\n");
		map_restart(false);
		return;
	}
	wait 0.05;
	players = getentarray("player", "classname");
	logPrint("PROBE players " + players.size + "\n");
	for (i = 0; i < players.size; i++)
		logPrint("PROBE player " + players[i] getEntityNumber() + " name " + players[i].name + " state " + players[i].sessionstate + " team " + players[i].sessionteam + "\n");
	wait 5;
	ent = addtestclient();
	logPrint("PROBE added2 num " + ent getEntityNumber() + " name " + ent.name + "\n");
	setcvar("probe_tc_pass", "");
	logPrint("PROBE done\n");
}

onconnect()
{
	logPrint("PROBE connect " + self getEntityNumber() + " " + self.name + " state " + self.sessionstate + " t " + getTime() + "\n");
	self waittill("begin");
	logPrint("PROBE begin " + self getEntityNumber() + " t " + getTime() + "\n");
}
