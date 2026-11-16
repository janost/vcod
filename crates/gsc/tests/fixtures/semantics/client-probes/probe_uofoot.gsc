//	United Offensive on-foot capture's server half (COD_VERSION=uo
//	tools/run_probe.sh, mp_foy, a --save-slope --probe-uofoot client).
//	Under probe_teleport 1 it puts every allied player at one spot, once
//	per spawn, facing the mount, and keeps three frags in the grenade
//	slot. probe_spot stand faces the hmgstanding sandbags at (-85 2700),
//	crouch the hmgcrouching ones at (-1080 -3313), and mount open ground
//	in front of a script trigger_mount spawned crouched
//	(spawnTriggerMount), which the client sees only as serverCursorHint 7.
//	probe_fires 1 logs each of its "trigger" notifies. probe_spot mound
//	(mp_carentan) is probe_prone's ~19 degree mound at (-224 60) facing
//	uphill, a standing spawnTriggerMount 15 ahead of it, for the deploy
//	pitch clamp (--probe-pronemove deploy).
//	docs/research/coduo-movement.md, sections 7 and 11.

main()
{
	thread watch_teleports();
	maps\mp\gametypes\dm::main();
}

watch_teleports()
{
	if (getcvar("probe_teleport") != "1")
		return;
	want = "mp_foy";
	if (getcvar("probe_spot") == "mound")
		want = "mp_carentan";
	if (getcvar("mapname") != want)
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	if (getcvar("probe_spot") == "mound")
	{
		//	Round the point 15 ahead of the spot along yaw 270, the
		//	ground's height there and above.
		spawnTriggerMount((-244, 30, -20), (-204, 52, 120), 0);
		logPrint("PROBE mount spawned\n");
	}
	if (getcvar("probe_spot") == "mount")
	{
		//	15 ahead of the spot and 1 up is inside it (G_CheckForCursorHints).
		m = spawnTriggerMount((-130, 2598, -20), (-70, 2640, 40), 1);
		logPrint("PROBE mount spawned\n");
		if (getcvar("probe_fires") == "1")
			m thread log_fires();
	}
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_place();
		wait 0.05;
	}
}

//	Under probe_fires 1, one line per "trigger" the mount raises, on the
//	clock its waiter woke at.
log_fires()
{
	for (;;)
	{
		self waittill("trigger", other);
		logPrint("PROBE fire " + getTime() + "\n");
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_place()
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
	{
		self.probe_placed = undefined;
		return;
	}
	if (self.pers["team"] != "allies")
		return;
	//	A thrown frag goes back in, so every cook step has one.
	if (self getWeaponSlotClipAmmo("grenade") < 3)
	{
		self setWeaponSlotWeapon("grenade", "fraggrenade_mp");
		self setWeaponSlotClipAmmo("grenade", 3);
	}
	if (isdefined(self.probe_placed))
		return;
	self.probe_placed = 1;
	spot = getcvar("probe_spot");
	if (spot == "crouch")
	{
		origin = (-1094, -3293, 8);
		yaw = 0;
	}
	else if (spot == "mound")
	{
		//	probe_prone's mound spot, a few units above the ground.
		origin = (-224, 60, 45);
		yaw = 270;
	}
	else if (spot == "mount")
	{
		origin = (-100, 2590, 8);
		yaw = 90;
	}
	else
	{
		origin = (7.5, 2686, 8);
		yaw = 90;
	}
	self setorigin(origin);
	self setplayerangles((0, yaw, 0));
	logPrint("PROBE place " + getTime() + " " + self getEntityNumber() + " " + origin + " " + yaw + "\n");
}
