//	linkTo's second round, mp_carentan only: enableLinkTo on the bombzone_A
//	trigger_multiple, linkTo on a misc_mg42, a model change on a tag parent
//	(G_UpdateTagInfoOfChildren) and items unlinked in mid-air. Once an
//	allied player has spawned it parks the player and logs every server frame
//	of a phase:
//
//		PROBE g <phase> <time> <origin> <angles>
//		PROBE m <phase> <time> <parent origin> <k1 origin> <k1 angles> <k0 origin>
//		PROBE n <tag> <time> <toucher>
//
//	spawn("trigger_radius", ...) is not a way in: retail refuses it with
//	'unable to spawn "trigger_radius" entity'.
//
//	Ends on a deliberate fatal (enableLinkTo on a script_origin), so the last
//	PROBE_FATAL line is part of the measurement.
//	Run by tools/run_probe.sh with a --probe-team allies client.

main()
{
	precacheModel("xmodel/playerbody_german_wehrmacht");
	precacheModel("xmodel/weapon_thompson");
	trigs = getentarray("trigger_multiple", "classname");
	for (i = 0; i < trigs.size; i++)
		trigs[i].script_gameobjectname = "dm";
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
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_run();
		wait 0.05;
	}
}

try_run()
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
	self thread phases();
}

phases()
{
	wait 3;
	self setorigin((-512, 2688, -16));
	self setplayerangles((0, 90, 0));
	wait 1;
	logPrint("PROBE player " + self getEntityNumber() + " " + self.model + "\n");
	//	Model *4's box is (-84 -112 -48) to (106 112 8) about the origin, so
	//	an origin 16 above the feet puts the player inside it.
	at = self.origin + (0, 0, 16);

	//	The trigger_multiple unlinked, moved by an origin write: the wait
	//	key's gate, for comparison with the linked run below.
	t = getent("bombzone_A", "targetname");
	home = t.origin;
	logPrint("PROBE ents t " + t getEntityNumber() + " " + home + "\n");
	t thread notes("t");
	logPrint("PROBE at trig_static " + getTime() + "\n");
	t.origin = at;
	sample1(t, "trig_static", 1.5);
	t.origin = home;
	wait 0.5;

	//	enableLinkTo, then a link to a script_origin that carries the trigger
	//	onto the player, parks it there and takes it home.
	t enableLinkTo();
	p = spawn("script_origin", home);
	t linkto(p);
	logPrint("PROBE at trig_link " + getTime() + "\n");
	p moveto(at, 1);
	sample1(t, "trig_link", 2.5);
	logPrint("PROBE at trig_home " + getTime() + "\n");
	p moveto(home, 1);
	sample1(t, "trig_home", 1.5);
	t unlink();
	p delete();

	//	A misc_mg42 linked to a script_origin at its own origin.
	mgs = getentarray("misc_mg42", "classname");
	mg = undefined;
	for (i = 0; i < mgs.size; i++)
		if (!isdefined(mg) || mgs[i] getEntityNumber() < mg getEntityNumber())
			mg = mgs[i];
	mp = spawn("script_origin", mg.origin);
	logPrint("PROBE ents mg " + mg getEntityNumber() + " " + mg.origin + " " + mg.angles + "\n");
	mg linkto(mp);
	logPrint("PROBE at mg_up " + getTime() + "\n");
	mp movez(32, 1);
	sample1(mg, "mg_up", 1.5);
	logPrint("PROBE at mg_yaw " + getTime() + "\n");
	mp rotateyaw(45, 1);
	sample1(mg, "mg_yaw", 1.5);
	mg unlink();
	logPrint("PROBE at mg_unlinked " + getTime() + "\n");
	mp movez(-32, 1);
	sample1(mg, "mg_unlinked", 1.5);
	mp delete();

	//	A tag parent's model change: k1 rides "bip01 head" with zero offsets,
	//	k0 the model's own frame.
	m = spawn("script_model", self.origin + (64, -128, 0));
	m setmodel(self.model);
	k1 = spawn("script_origin", m.origin);
	k0 = spawn("script_origin", m.origin + (0, 32, 0));
	k1 linkto(m, "bip01 head", (0, 0, 0), (0, 0, 0));
	k0 linkto(m);
	logPrint("PROBE ents m " + m getEntityNumber() + " k1 " + k1 getEntityNumber() + " k0 " + k0 getEntityNumber() + "\n");
	logPrint("PROBE at tag_first " + getTime() + "\n");
	m movez(16, 1);
	sample_tag("tag_first", m, k1, k0, 1.2);
	//	Another body with the same bone.
	logPrint("PROBE at tag_same " + getTime() + "\n");
	m setmodel("xmodel/playerbody_german_wehrmacht");
	m movez(-16, 1);
	sample_tag("tag_same", m, k1, k0, 1.2);
	//	A model without the bone.
	logPrint("PROBE at tag_none " + getTime() + "\n");
	m setmodel("xmodel/weapon_thompson");
	m movez(16, 1);
	sample_tag("tag_none", m, k1, k0, 1.2);
	//	Back to a body: does k1 come back?
	logPrint("PROBE at tag_back " + getTime() + "\n");
	m setmodel(self.model);
	m movez(-16, 1);
	sample_tag("tag_back", m, k1, k0, 1.2);
	//	No model at all, with k1 linked again.
	k1 linkto(m, "bip01 head", (0, 0, 0), (0, 0, 0));
	logPrint("PROBE at tag_empty " + getTime() + "\n");
	m setmodel("");
	m movez(16, 1);
	sample_tag("tag_empty", m, k1, k0, 1.2);
	k0 delete();
	k1 delete();
	m delete();

	//	An item linked the frame it spawns in mid-air, carried up and
	//	unlinked; then one that has landed, lifted and unlinked.
	ip = spawn("script_origin", self.origin + (96, 0, 100));
	it = spawn("item_health", self.origin + (96, 0, 100));
	it linkto(ip);
	logPrint("PROBE ents it " + it getEntityNumber() + "\n");
	logPrint("PROBE at item_air " + getTime() + "\n");
	ip movez(20, 1);
	sample1(it, "item_air", 1.5);
	it unlink();
	logPrint("PROBE at item_air_free " + getTime() + "\n");
	sample1(it, "item_air_free", 2);
	it delete();
	ip delete();

	it = spawn("item_health", self.origin + (96, 0, 1));
	wait 1;
	ip = spawn("script_origin", it.origin);
	it linkto(ip);
	logPrint("PROBE at item_lift " + getTime() + "\n");
	ip movez(60, 1);
	sample1(it, "item_lift", 1.5);
	it unlink();
	logPrint("PROBE at item_lift_free " + getTime() + "\n");
	sample1(it, "item_lift_free", 2);
	it delete();
	ip delete();

	//	The bit is already set on every script_origin.
	s = spawn("script_origin", self.origin);
	logPrint("PROBE at enable_twice " + getTime() + "\n");
	s enableLinkTo();
	logPrint("PROBE enable_twice_survived\n");
}

notes(tag)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", who);
		logPrint("PROBE n " + tag + " " + getTime() + " " + who getEntityNumber() + "\n");
	}
}

sample1(e, phase, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE g " + phase + " " + getTime() + " " + e.origin + " " + e.angles + "\n");
		wait 0.05;
	}
}

sample_tag(phase, m, k1, k0, secs)
{
	frames = secs * 20;
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE m " + phase + " " + getTime() + " " + m.origin + " " + k1.origin + " " + k1.angles + " " + k0.origin + "\n");
		wait 0.05;
	}
}
