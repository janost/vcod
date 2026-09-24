//	Item pickup's server half: every "touch" and "trigger" notify an item or
//	a player takes, one logPrint each, for the A/B in
//	crates/server/tests/pickup_ab.rs. Run by tools/run_probe.sh with a
//	--save-pickup client; the client half writes its own fixture.

main()
{
	level.probe_taken = [];
	thread watch_entities();
	thread watch_players();
	thread watch_teleports();
	maps\mp\gametypes\dm::main();
}

//	The classnames the capture handles: mp_carentan's two placed kinds, the
//	allied loadout a swap drops, and the health dm drops on a death.
is_watched(cls)
{
	if (cls == "mpweapon_fg42" || cls == "mpweapon_panzerfaust")
		return true;
	if (cls == "mpweapon_m1carbine" || cls == "mpweapon_colt" || cls == "mpweapon_fraggrenade")
		return true;
	return cls == "item_health";
}

//	Every entity once. A watched one gets its two notify threads; any other
//	new entity but a player is logged as "other", which is how a dropped
//	weapon whose classname is not in the list still shows up.
watch_entities()
{
	wait 1;
	level.probe_census = 1;
	for (;;)
	{
		ents = getentarray();
		for (i = 0; i < ents.size; i++)
			ents[i] consider();
		level.probe_census = 0;
		wait 0.05;
	}
}

consider()
{
	if (isdefined(self.probe_seen))
		return;
	self.probe_seen = 1;
	if (!isdefined(self.classname))
		return;
	num = self getEntityNumber();
	if (is_watched(self.classname))
	{
		logPrint("PROBE item " + num + " " + self.classname + " " + getTime() + " " + self.origin + "\n");
		self thread watch_touch(num);
		self thread watch_trigger(num);
		return;
	}
	if (level.probe_census || self.classname == "player")
		return;
	logPrint("PROBE other " + num + " " + self.classname + " " + getTime() + "\n");
}

watch_touch(num)
{
	self endon("death");
	for (;;)
	{
		self waittill("touch", other);
		logPrint("PROBE touch " + num + " " + getTime() + " " + other getEntityNumber() + "\n");
	}
}

watch_trigger(num)
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", player, swapped);
		logPrint("PROBE trigger " + num + " " + getTime() + " " + player getEntityNumber() + " " + describe(swapped) + "\n");
		level.probe_taken[num] = 1;
	}
}

describe(e)
{
	if (!isdefined(e))
		return "undefined";
	return e getEntityNumber() + ":" + e.classname;
}

watch_players()
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] watch_player();
		wait 0.05;
	}
}

watch_player()
{
	if (isdefined(self.probe_watched))
		return;
	self.probe_watched = 1;
	self thread watch_player_touch(self getEntityNumber());
}

watch_player_touch(num)
{
	self endon("disconnect");
	for (;;)
	{
		self waittill("touch", item);
		logPrint("PROBE ptouch " + num + " " + getTime() + " " + item getEntityNumber() + "\n");
	}
}

//	mp_carentan's two fg42s are a town apart, so with `probe_teleport 1` each
//	live player is put on the first once, and on the second two seconds after
//	the first has been taken. The A/B gate places its client itself and never
//	sets the cvar; an unset cvar reads "".
watch_teleports()
{
	if (getcvar("probe_teleport") != "1")
		return;
	wait 1;
	fg = getentarray("mpweapon_fg42", "classname");
	if (fg.size < 2)
	{
		logPrint("PROBE teleport unsupported " + getcvar("mapname") + "\n");
		return;
	}
	level.probe_tp = [];
	first = fg[0] getEntityNumber();
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
			players[i] try_teleport(fg, first);
		wait 0.05;
	}
}

//	Early returns rather than one compound test: `self.sessionstate` is
//	undefined before the first spawn, and comparing it to a string is not
//	something to lean on either VM short-circuiting past.
try_teleport(fg, first)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
		return;
	if (!isalive(self))
		return;
	num = self getEntityNumber();
	if (!isdefined(level.probe_tp[num]))
	{
		level.probe_tp[num] = 1;
		self setOrigin(fg[0].origin);
		logPrint("PROBE teleport " + num + " 1 " + fg[0].origin + "\n");
		return;
	}
	if (level.probe_tp[num] != 1 || !isdefined(level.probe_taken[first]))
		return;
	level.probe_tp[num] = 2;
	wait 2;
	self setOrigin(fg[1].origin);
	logPrint("PROBE teleport " + num + " 2 " + fg[1].origin + "\n");
}
