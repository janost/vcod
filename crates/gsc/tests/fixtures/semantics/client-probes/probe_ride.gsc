//	Mover push and ride capture's server half, mp_carentan only. Keeps the
//	bombzone_A brush model *5 (dm's _gameobjects would delete it), puts every
//	allied player on its floor slab (brush 4281, top z -22) under
//	probe_teleport 1, and runs one verb per phase on the brush model, then on a
//	script_origin the player is linked to, and last hides, shows, notsolids,
//	solids and deletes the brush model. Every server frame of a phase logs
//	the player's origin and the mover's getorigin()/.angles:
//
//		PROBE f <phase> <time> <player origin> <mover origin> <mover angles>
//
//	Run by tools/run_probe.sh with a --probe-ride --probe-team allies client,
//	which is the wire half: the playerstate per snapshot and every trajectory
//	change.

main()
{
	//	_gameobjects deletes every entity whose script_gameobjectname dm does
	//	not list; renaming the two bombzone brush models keeps them linked.
	bms = getentarray("script_brushmodel", "classname");
	for (i = 0; i < bms.size; i++)
		bms[i].script_gameobjectname = "dm";
	thread watch_players();
	maps\mp\gametypes\dm::main();
}

watch_players()
{
	if (getcvar("probe_teleport") != "1")
		return;
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	bms = getentarray("script_brushmodel", "classname");
	bz = undefined;
	for (i = 0; i < bms.size; i++)
	{
		logPrint("PROBE bm " + bms[i] getEntityNumber() + " " + bms[i].model + " " + bms[i] getorigin() + " " + bms[i].angles + "\n");
		//	`.model` reads "" on retail; *5 is the lower of the two numbers
		//	(docs/research/cod11-gsc-object-model.md 23.2).
		if (!isdefined(bz) || bms[i] getEntityNumber() < bz getEntityNumber())
			bz = bms[i];
	}
	if (!isdefined(bz))
	{
		logPrint("PROBE no_bz\n");
		return;
	}
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_ride(bz);
		wait 0.05;
	}
}

//	Early returns rather than one compound test: sessionstate is undefined
//	before the first spawn.
try_ride(bz)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_riding))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_riding = 1;
	self thread phases(bz);
}

//	One phase: place the player, wait for it to settle, start the verb, and
//	sample every frame until `secs` after the call.
place(spot, yaw)
{
	self setorigin(spot);
	self setplayerangles((0, yaw, 0));
	wait 1;
}

phases(bz)
{
	wait 3;
	logPrint("PROBE player " + self getEntityNumber() + "\n");

	//	Standing on the slab (the diagonal plank brush 4281, clear of the crate
	//	brushes above it): up, down, sideways, a small yaw about the brush
	//	model's own origin (the world origin, it has no origin brush) and back.
	self place((-215, 2463, -21), 90);
	logPrint("PROBE at ride_up " + getTime() + "\n");
	bz movez(48, 2);
	self sample("ride_up", bz, 3);

	logPrint("PROBE at ride_down " + getTime() + "\n");
	bz movez(-48, 2);
	self sample("ride_down", bz, 3);

	logPrint("PROBE at ride_x " + getTime() + "\n");
	bz movex(48, 2);
	self sample("ride_x", bz, 3);

	logPrint("PROBE at ride_x_back " + getTime() + "\n");
	bz movex(-48, 2);
	self sample("ride_x_back", bz, 3);

	logPrint("PROBE at ride_yaw " + getTime() + "\n");
	bz rotateyaw(2, 1);
	self sample("ride_yaw", bz, 2);

	logPrint("PROBE at ride_yaw_back " + getTime() + "\n");
	bz rotateyaw(-2, 1);
	self sample("ride_yaw_back", bz, 2);

	//	South of the slab on the ground, in its path: the push.
	self place((-215, 2430, -31), 90);
	logPrint("PROBE at push_y " + getTime() + "\n");
	bz movey(-48, 2);
	self sample("push_y", bz, 3);

	logPrint("PROBE at push_y_back " + getTime() + "\n");
	bz movey(48, 2);
	self sample("push_y_back", bz, 3);

	//	The slab lifted over the player's head and lowered onto it: the
	//	blocked push. Then back to the brush model's spawn origin.
	self place((-215, 2430, -31), 90);
	logPrint("PROBE at lift " + getTime() + "\n");
	bz movez(80, 0.5);
	self sample("lift", bz, 1);
	bz movey(-48, 0.5);
	self sample("over", bz, 1);
	logPrint("PROBE at crush " + getTime() + "\n");
	bz movez(-80, 2);
	self sample("crush", bz, 4);
	self place((-512, 2688, -16), 90);
	bz moveto((0, 0, 0), 1);
	self sample("reset", bz, 2);

	//	Linked to a script_origin 32 units in front, on the attackers' spawn
	//	clear of the slab: up, a quarter turn, and sideways.
	org = spawn("script_origin", self.origin + (32, 0, 0));
	self linkto(org);
	logPrint("PROBE at link_up " + getTime() + "\n");
	org movez(48, 2);
	self sample("link_up", org, 3);

	logPrint("PROBE at link_yaw " + getTime() + "\n");
	org rotateyaw(90, 2);
	self sample("link_yaw", org, 3);

	logPrint("PROBE at link_x " + getTime() + "\n");
	org movex(48, 2);
	self sample("link_x", org, 3);

	self unlink();
	org delete();

	//	Back beside the slab, in range of it: what hide, notsolid and delete
	//	do to the brush model's entity on the wire.
	self place((-215, 2430, -31), 90);
	logPrint("PROBE at bm_hide " + getTime() + "\n");
	bz hide();
	self sample("bm_hide", bz, 1);
	logPrint("PROBE at bm_show " + getTime() + "\n");
	bz show();
	self sample("bm_show", bz, 1);
	logPrint("PROBE at bm_notsolid " + getTime() + "\n");
	bz notsolid();
	self sample("bm_notsolid", bz, 1);
	logPrint("PROBE at bm_solid " + getTime() + "\n");
	bz solid();
	self sample("bm_solid", bz, 1);
	logPrint("PROBE at bm_delete " + getTime() + "\n");
	bz delete();
	wait 1;
	logPrint("PROBE done " + getTime() + "\n");
}

sample(phase, ent, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE f " + phase + " " + getTime() + " " + self.origin + " " + ent getorigin() + " " + ent.angles + "\n");
		wait 0.05;
	}
}
