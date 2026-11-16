//	What United Offensive's mp_arnhem dom props clip: the flag4stuff_allies
//	and flag4stuff_axis script_brushmodels (*17, *18) and flag2stuff's *121
//	and *122 are lump-24 terrain alone, no brushes, and dom.gsc only hides
//	and shows them. A bulletTrace straight down 400 units from spots over
//	their top faces, as loaded, after every flag*stuff entity is hidden,
//	after each is also made notsolid, and after both are undone. No
//	`_gameobjects` pass, so every gametype's entities stay. Run by
//	tools/run_probe.sh with COD_VERSION=uo on mp_arnhem; no client needed.
//	See this directory's README.md, "probe_domprops".

main()
{
	level.ceasefire = 0;
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

Callback_StartGameType()
{
	thread probe();
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}

probe()
{
	wait 1;
	spots = [];
	spots[spots.size] = (1262, 2802, 260);
	spots[spots.size] = (1200, 2776, 240);
	spots[spots.size] = (1266, 2804, 240);
	spots[spots.size] = (-1940, 126, 120);
	props = [];
	names = [];
	names[0] = "flag4stuff_allies";
	names[1] = "flag4stuff_axis";
	names[2] = "flag2stuff_allies";
	names[3] = "flag2stuff_axis";
	for (n = 0; n < names.size; n++)
	{
		ents = getentarray(names[n], "targetname");
		for (i = 0; i < ents.size; i++)
		{
			logPrint("PROBE prop " + names[n] + " " + ents[i] getEntityNumber() + " " + ents[i].model + "\n");
			props[props.size] = ents[i];
		}
	}
	traces("loaded", spots);
	for (i = 0; i < props.size; i++)
		props[i] hide();
	wait 0.1;
	traces("hidden", spots);
	for (i = 0; i < props.size; i++)
		props[i] notsolid();
	wait 0.1;
	traces("notsolid", spots);
	for (i = 0; i < props.size; i++)
	{
		props[i] solid();
		props[i] show();
	}
	wait 0.1;
	traces("restored", spots);
	logPrint("PROBE done\n");
}

traces(phase, spots)
{
	for (i = 0; i < spots.size; i++)
	{
		s = spots[i];
		t = bulletTrace(s, s - (0, 0, 400), false, undefined);
		e = "none";
		if (isdefined(t["entity"]))
			e = t["entity"] getEntityNumber() + " " + t["entity"].model;
		logPrint("PROBE " + phase + " " + s + " " + t["fraction"] + " " + t["position"] + " " + e + "\n");
	}
}
