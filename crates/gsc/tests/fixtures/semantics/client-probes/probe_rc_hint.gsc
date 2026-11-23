//	How wide a retail client draws the cursor hint's icon for an unowned
//	and an owned weapon (docs/research/cod11-hud-protocol.md 9.12;
//	cod11-items.md 12.3 for the numbering), the server half of
//	tools/capture/retail-client steps `hint` (1.1) and `hint-uo` (UO).
//	Stock dm on 1.1, 1.5 or UO.
//
//	Every client that reaches the spectator state is put on allies, spawned
//	through dm's own spawnPlayer and left holding the m1 carbine
//	(wideListIcon 1) with the colt (0) as his other weapon, neither with
//	reserve ammo, so their items still hint as ammo. He is held on
//	the first deathmatch spawn with floor 48 units along its facing, his
//	view on a point 8 above that floor spot, every frame:
//
//	PROBE site <time> <origin> <view>
//
//	`rc_phase` (set from the server console) puts one weapon item,
//	suspended and hidden, on that spot; the hint the server finds there is
//	the playerstate's, which the client draws whatever it shows of the
//	item. Phases: `none` (no item, the default), `unowned_wide`
//	(kar98k: 9 + w on 1.1), `owned_wide` (m1 carbine: 73 + w),
//	`unowned_narrow` (luger), `owned_narrow` (colt). Each player's `rc_shot`
//	cvar names the phase's three screenshots (`rc_<phase>_<n>`). Each phase
//	is logged with
//	the item's entity number (-1 for none):
//
//	PROBE phase <time> <phase> <item>

main()
{
	thread autojoin("m1carbine_mp");
	thread phases();
	maps\mp\gametypes\dm::main();
}

//	dm's connect leaves a client spectating with its team menu open; this
//	answers for it.
autojoin(weapon)
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (!isdefined(p.sessionstate) || p.sessionstate != "spectator")
				continue;
			if (isdefined(p.rc_joined))
				continue;
			p.rc_joined = true;
			p.pers["team"] = "allies";
			p.pers["weapon"] = weapon;
			p closeMenu();
			p maps\mp\gametypes\dm::spawnPlayer();
			p thread hold();
		}
		wait 0.05;
	}
}

hold()
{
	self endon("disconnect");
	self takeAllWeapons();
	self giveWeapon("m1carbine_mp");
	self giveWeapon("colt_mp");
	// No reserve: an item the grab test refuses (full ammo) hints nothing
	// (cod11-items.md 12.3).
	self setWeaponSlotAmmo("primary", 0);
	self setWeaponSlotAmmo("pistol", 0);
	self setSpawnWeapon("m1carbine_mp");
	site();
	logPrint("PROBE site " + getTime() + " " + level.rc_site + " " + level.rc_view + "\n");
	self shots(getcvar("rc_phase"));
	for (;;)
	{
		self setOrigin(level.rc_site);
		self setPlayerAngles(level.rc_view);
		wait 0.05;
	}
}

//	The first deathmatch spawn with a clear line to floor 48 units along
//	its yaw; the item goes on that floor.
site()
{
	if (isdefined(level.rc_site))
		return;
	spots = getentarray("mp_deathmatch_spawn", "classname");
	for (k = 0; k < spots.size; k++)
	{
		at = spots[k].origin;
		yaw = spots[k].angles[1];
		ahead = anglesToForward((0, yaw, 0));
		spot = at + (ahead[0] * 48, ahead[1] * 48, 0);
		floor = bulletTrace(spot + (0, 0, 32), spot + (0, 0, -64), false, undefined);
		eye = at + (0, 0, 60);
		if (floor["fraction"] == 1)
			continue;
		item = floor["position"] + (0, 0, 8);
		line = bulletTrace(eye, item, false, undefined);
		if (line["fraction"] < 1)
			continue;
		level.rc_site = at;
		level.rc_item = item;
		level.rc_view = vectorToAngles(item - eye);
		return;
	}
	logPrint("PROBE no site\n");
	level.rc_site = spots[0].origin;
	level.rc_item = level.rc_site + (48, 0, 8);
	level.rc_view = (45, 0, 0);
}

phases()
{
	setcvar("rc_phase", "none");
	phase = "none";
	item = undefined;
	logPrint("PROBE phase " + getTime() + " none -1\n");
	for (;;)
	{
		wait 0.05;
		next = getcvar("rc_phase");
		if (next == phase || !isdefined(level.rc_item))
			continue;
		phase = next;
		if (isdefined(item))
			item delete();
		item = undefined;
		name = undefined;
		if (phase == "unowned_wide")
			name = "mpweapon_kar98k";
		else if (phase == "owned_wide")
			name = "mpweapon_m1carbine";
		else if (phase == "unowned_narrow")
			name = "mpweapon_luger";
		else if (phase == "owned_narrow")
			name = "mpweapon_colt";
		num = -1;
		if (isdefined(name))
		{
			item = spawn(name, level.rc_item - (0, 0, 8), 1);
			item hide();
			num = item getEntityNumber();
		}
		logPrint("PROBE phase " + getTime() + " " + phase + " " + num + "\n");
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] shots(phase);
	}
}

//	The client's `rc_shot`: three named screenshots of `phase`, which the
//	capture's console line `vstr rc_shot` runs.
shots(phase)
{
	s = "screenshot rc_" + phase + "_1; wait 25; screenshot rc_" + phase + "_2; wait 25; screenshot rc_" + phase + "_3";
	self setClientCvar("rc_shot", s);
}
