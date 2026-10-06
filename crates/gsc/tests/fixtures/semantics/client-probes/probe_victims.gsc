//	Four leftovers of the damage path, on four clients under sd rules. Which
//	inflictor finishPlayerDamage hands the killed callback; what a spawn does
//	to health; how far a dead player slides from a blast's knockback; what a
//	blast does to a turret, and where its notifies fall among the player
//	callbacks of the same walk. Recipe: README.md.

main()
{
	thread drive();
	maps\mp\gametypes\sd::main();
}

num(e)
{
	if (!isdefined(e))
		return "undefined";
	return e getEntityNumber();
}

wrapDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE cb " + self getEntityNumber() + " " + iDamage + " " + self.sessionstate + " " + self.health + "\n");
	[[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
}

wrapKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc)
{
	logPrint("PROBE killed " + self getEntityNumber() + " inflictor " + num(eInflictor) + " attacker " + num(eAttacker) + " " + sMeansOfDeath + "\n");
	[[level.probe_killed]](eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc);
}

respawn(p, at)
{
	p.spawned = undefined;
	p maps\mp\gametypes\sd::spawnPlayer();
	p setorigin(at);
}

watchTurret(t)
{
	t endon("probe_done");
	for (;;)
	{
		t waittill("damage", a, b);
		logPrint("PROBE tdamage " + a + " " + num(b) + " health " + t.health + "\n");
	}
}

watchTurretDeath(t)
{
	t endon("probe_done");
	for (;;)
	{
		t waittill("death", a);
		logPrint("PROBE tdeath " + num(a) + " health " + t.health + "\n");
	}
}

drive()
{
	wait 0.05;
	if (!game["matchstarted"])
		return;
	for (;;)
	{
		players = getentarray("player", "classname");
		n = 0;
		for (i = 0; i < players.size; i++)
		{
			if (players[i].sessionstate == "playing")
				n++;
		}
		if (n >= 4)
			break;
		wait 0.05;
	}
	wait 2;
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::wrapDamage;
	level.probe_killed = level.callbackPlayerKilled;
	level.callbackPlayerKilled = ::wrapKilled;

	blast = (-176.8, 2473.1, 7);
	front = (-226, 2424, -32);
	back = (-269, 2381, -32);
	park0 = (400, 3272, -23.875);
	park1 = (224, -1280, 1.86);
	park2 = (300, 3272, -23.875);
	park3 = (324, -1280, 1.86);
	players = getentarray("player", "classname");
	players[0] setorigin(park0);
	players[1] setorigin(park1);
	players[2] setorigin(park2);
	players[3] setorigin(park3);
	org = spawn("script_origin", (0, 0, 0));
	logPrint("PROBE org " + num(org) + "\n");
	wait 1;

	// finishPlayerDamage's inflictor.
	logPrint("PROBE fpd ent_player\n");
	players[0] finishPlayerDamage(org, players[1], 500, 0, "MOD_GRENADE_SPLASH", "fraggrenade_mp", players[0].origin, (0, 0, 1), "none");
	logPrint("PROBE fpd player_ent\n");
	players[3] finishPlayerDamage(players[2], org, 500, 0, "MOD_RIFLE_BULLET", "kar98k_mp", players[3].origin, (0, 0, 1), "none");
	wait 0.5;
	respawn(players[0], park0);
	respawn(players[3], park3);
	wait 0.5;
	logPrint("PROBE fpd undefined_player\n");
	players[0] finishPlayerDamage(undefined, players[1], 500, 0, "MOD_RIFLE_BULLET", "kar98k_mp", players[0].origin, (0, 0, 1), "none");
	wait 0.5;
	respawn(players[0], park0);
	wait 1;

	// What a spawn does to health.
	players[2].health = 37;
	players[2] maps\mp\gametypes\sd::spawnSpectator();
	logPrint("PROBE spec health " + players[2].health + " " + players[2].sessionstate + "\n");
	wait 0.5;
	logPrint("PROBE spec_later health " + players[2].health + "\n");
	players[2].health = 41;
	players[2].sessionstate = "playing";
	players[2] spawn(park2, (0, 0, 0));
	logPrint("PROBE play health " + players[2].health + " maxhealth " + players[2].maxhealth + "\n");
	wait 0.5;
	logPrint("PROBE play_later health " + players[2].health + "\n");
	respawn(players[2], park2);
	wait 1;

	// A dead player's slide, and a live one's beside it.
	players[0].health = 100;
	players[1].health = 1000;
	players[0] setorigin(front);
	players[1] setorigin(back);
	wait 1;
	logPrint("PROBE slide_before 0 " + players[0].origin + " 1 " + players[1].origin + "\n");
	radiusDamage(blast, 500, 200, 200);
	for (i = 0; i < 30; i++)
	{
		logPrint("PROBE slide " + i + " 0 " + players[0].sessionstate + " " + players[0].origin + " 1 " + players[1].origin + "\n");
		wait 0.05;
	}
	respawn(players[0], park0);
	players[1] setorigin(park1);
	wait 1;

	// A turret as a blast victim, beside a player.
	turrets = getentarray("misc_mg42", "classname");
	t = undefined;
	for (i = 0; i < turrets.size; i++)
	{
		logPrint("PROBE turret " + num(turrets[i]) + " " + turrets[i].origin + " health " + turrets[i].health + "\n");
		if (turrets[i].origin[0] > 1000)
			t = turrets[i];
	}
	if (!isdefined(t))
	{
		logPrint("PROBE done\n");
		return;
	}
	t thread watchTurret(t);
	t thread watchTurretDeath(t);
	players[1].health = 1000;
	players[1] setorigin((1712, 1990, 20));
	wait 1;
	logPrint("PROBE tplace 1 " + players[1].origin + "\n");
	tblast = (1712, 1900, 40);
	for (i = 0; i < 3; i++)
	{
		logPrint("PROBE tblast " + i + "\n");
		radiusDamage(tblast, 300, 60, 60);
		logPrint("PROBE tblast_after " + i + " health " + t.health + "\n");
		wait 0.5;
	}
	t notify("probe_done");
	logPrint("PROBE done\n");
}
