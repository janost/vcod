//	linkTo on script entities, mp_carentan only. Keeps the bombzone_A brush
//	model *5 the way probe_ride does, and once an allied player has spawned
//	links script_origins and a script_model to it, to each other and to the
//	player, moves the parents and logs every server frame:
//
//		PROBE f <phase> <time> <parent origin> <parent angles> <a origin> <a angles> <b origin> <b angles>
//		PROBE g <phase> <time> <a origin> <a angles>
//
//	Ends on a deliberate fatal (a link cycle), so the last PROBE_FATAL line is
//	part of the measurement.
//	Run by tools/run_probe.sh with a --probe-team allies client.

main()
{
	bms = getentarray("script_brushmodel", "classname");
	for (i = 0; i < bms.size; i++)
		bms[i].script_gameobjectname = "dm";
	thread watch_players();
	maps\mp\gametypes\dm::main();
}

watch_players()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 1;
	bms = getentarray("script_brushmodel", "classname");
	bz = undefined;
	for (i = 0; i < bms.size; i++)
		if (!isdefined(bz) || bms[i] getEntityNumber() < bz getEntityNumber())
			bz = bms[i];
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_run(bz);
		wait 0.05;
	}
}

try_run(bz)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (isdefined(self.probe_running))
		return;
	if (self.pers["team"] != "allies")
		return;
	self.probe_running = 1;
	self thread phases(bz);
}

phases(bz)
{
	wait 3;
	self setorigin((-512, 2688, -16));
	self setplayerangles((0, 90, 0));
	wait 1;
	logPrint("PROBE player " + self getEntityNumber() + " " + self.model + "\n");

	//	Two children of the slab: a turned script_origin and a script_model
	//	with the player's own (precached) model, for the wire.
	a = spawn("script_origin", (-215, 2463, 40));
	a.angles = (0, 30, 0);
	b = spawn("script_model", (-200, 2400, -22));
	b setmodel(self.model);
	logPrint("PROBE ents bz " + bz getEntityNumber() + " a " + a getEntityNumber() + " b " + b getEntityNumber() + "\n");
	a linkto(bz);
	b linkto(bz);
	//	The link itself moves nothing.
	logPrint("PROBE linked " + getTime() + " " + a getorigin() + " " + a.angles + " " + b getorigin() + " " + b.angles + "\n");

	logPrint("PROBE at bm_up " + getTime() + "\n");
	bz movez(48, 2);
	sample("bm_up", bz, a, b, 3);

	logPrint("PROBE at bm_yaw " + getTime() + "\n");
	bz rotateyaw(10, 1);
	sample("bm_yaw", bz, a, b, 2);

	logPrint("PROBE at bm_yaw_back " + getTime() + "\n");
	bz rotateyaw(-10, 1);
	sample("bm_yaw_back", bz, a, b, 2);

	logPrint("PROBE at bm_x " + getTime() + "\n");
	bz movex(48, 2);
	sample("bm_x", bz, a, b, 3);

	//	A verb on a linked mover: does the link or the verb win, and does
	//	movedone still come?
	a thread done("verb_linked", "movedone");
	logPrint("PROBE at verb_linked " + getTime() + "\n");
	a movez(100, 1);
	sample("verb_linked", bz, a, b, 2);

	//	unlink mid-move: a stays where it was, b rides on.
	logPrint("PROBE at unlink " + getTime() + "\n");
	bz movez(-48, 2);
	thread unlink_later(a, 1);
	sample("unlink", bz, a, b, 3);

	logPrint("PROBE at bm_x_back " + getTime() + "\n");
	bz movex(-48, 2);
	sample("bm_x_back", bz, a, b, 3);
	b unlink();
	a delete();
	b delete();

	//	A chain whose parents come first in entity order, then one whose
	//	parents come last.
	c0 = spawn("script_origin", (-512, 2688, 100));
	c1 = spawn("script_origin", (-480, 2688, 100));
	c2 = spawn("script_origin", (-480, 2720, 100));
	c1.angles = (0, 45, 0);
	c1 linkto(c0);
	c2 linkto(c1);
	logPrint("PROBE ents chain_fwd " + c0 getEntityNumber() + " " + c1 getEntityNumber() + " " + c2 getEntityNumber() + "\n");
	logPrint("PROBE at chain_fwd " + getTime() + "\n");
	c0 movez(48, 1);
	sample("chain_fwd", c0, c1, c2, 1.5);
	logPrint("PROBE at chain_fwd_yaw " + getTime() + "\n");
	c0 rotateyaw(90, 1);
	sample("chain_fwd_yaw", c0, c1, c2, 1.5);
	c2 delete();
	c1 delete();
	c0 delete();
	wait 0.2;

	r2 = spawn("script_origin", (-480, 2720, 100));
	r1 = spawn("script_origin", (-480, 2688, 100));
	r0 = spawn("script_origin", (-512, 2688, 100));
	r1.angles = (0, 45, 0);
	r2 linkto(r1);
	r1 linkto(r0);
	logPrint("PROBE ents chain_rev " + r0 getEntityNumber() + " " + r1 getEntityNumber() + " " + r2 getEntityNumber() + "\n");
	logPrint("PROBE at chain_rev " + getTime() + "\n");
	r0 movez(48, 1);
	sample("chain_rev", r0, r1, r2, 1.5);
	logPrint("PROBE at chain_rev_yaw " + getTime() + "\n");
	r0 rotateyaw(90, 1);
	sample("chain_rev_yaw", r0, r1, r2, 1.5);
	r2 unlink();
	r1 unlink();
	r2 delete();
	r1 delete();
	r0 delete();

	//	The parent deleted mid-move: the child keeps the pose it had.
	p = spawn("script_origin", (-512, 2688, 100));
	k = spawn("script_origin", (-480, 2688, 100));
	k linkto(p);
	logPrint("PROBE at del_parent " + getTime() + "\n");
	p movez(48, 2);
	k thread sample1("del_parent", 2);
	wait 1;
	logPrint("PROBE deleting " + getTime() + "\n");
	p delete();
	wait 1.1;
	k delete();

	//	Linked to the player: no tag, a bone, a tag with zero offsets and the
	//	four-argument form with no tag.
	k1 = spawn("script_origin", self.origin + (32, 0, 40));
	k2 = spawn("script_origin", self.origin + (0, 0, 60));
	k3 = spawn("script_origin", self.origin);
	k4 = spawn("script_origin", self.origin);
	k1 linkto(self);
	k2 linkto(self, "bip01 head");
	k3 linkto(self, "tag_weapon_right", (0, 0, 0), (0, 0, 0));
	k4 linkto(self, "", (16, 0, 8), (0, 90, 0));
	logPrint("PROBE linked_pl " + getTime() + " " + k2 getorigin() + " " + k2.angles + " " + k3 getorigin() + " " + k3.angles + "\n");
	logPrint("PROBE at pl_still " + getTime() + "\n");
	sample_pl("pl_still", k1, k2, k3, k4, 1);
	logPrint("PROBE at pl_turn " + getTime() + "\n");
	self setplayerangles((0, 180, 0));
	sample_pl("pl_turn", k1, k2, k3, k4, 1);
	logPrint("PROBE at pl_move " + getTime() + "\n");
	self setorigin((-448, 2688, -16));
	sample_pl("pl_move", k1, k2, k3, k4, 1);
	k1 delete();
	k2 delete();
	k3 delete();
	k4 delete();

	//	A cycle on two model-less script_origins. The fatal's message is the
	//	measurement: which of linkTo's three errors a failed link reports.
	x = spawn("script_origin", (-512, 2688, 100));
	y = spawn("script_origin", (-480, 2688, 100));
	x linkto(y);
	logPrint("PROBE at cycle " + getTime() + "\n");
	y linkto(x);
	logPrint("PROBE cycle_survived\n");
}

unlink_later(e, secs)
{
	wait secs;
	logPrint("PROBE unlinking " + getTime() + "\n");
	e unlink();
}

done(tag, event)
{
	self endon("death");
	self waittill(event);
	logPrint("PROBE done " + tag + " " + event + " " + getTime() + "\n");
}

sample(phase, parent, a, b, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE f " + phase + " " + getTime() + " " + parent getorigin() + " " + parent.angles + " " + a getorigin() + " " + a.angles + " " + b getorigin() + " " + b.angles + "\n");
		wait 0.05;
	}
}

sample1(phase, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE g " + phase + " " + getTime() + " " + self getorigin() + " " + self.angles + "\n");
		wait 0.05;
	}
}

sample_pl(phase, k1, k2, k3, k4, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE f " + phase + " " + getTime() + " " + self.origin + " " + self.angles + " " + k1 getorigin() + " " + k1.angles + " " + k2 getorigin() + " " + k2.angles + "\n");
		logPrint("PROBE h " + phase + " " + getTime() + " " + k3 getorigin() + " " + k3.angles + " " + k4 getorigin() + " " + k4.angles + "\n");
		wait 0.05;
	}
}
