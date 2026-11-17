//	`scriptUsage`'s entity line, and the caret under a path call and a
//	method call through a pointer (docs/research/cod11-server-handshake.md,
//	"Console commands over rcon"). No gametype under it. `probe_mode` picks
//	what the script does to the map's entities, `probe_key` the key `del`
//	deletes by; the console command runs over rcon 5 s in.

main()
{
	level.callbackStartGameType = ::blank;
	level.callbackPlayerConnect = ::blank;
	level.callbackPlayerDisconnect = ::blank;
	level.callbackPlayerDamage = ::blank;
	level.callbackPlayerKilled = ::blank;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
	thread calls();
	thread run(getcvar("probe_mode"));
}

blank()
{
}

run(mode)
{
	wait 1;
	if (mode == "del")
	{
		ents = getentarray(getcvar("probe_value"), getcvar("probe_key"));
		for (i = 0; i < ents.size; i++)
			ents[i] delete();
		ents = undefined;
	}
	if (mode != "base" && mode != "del")
	{
		s = getentarray("mp_teamdeathmatch_spawn", "classname");
		level.s = s;
		if (mode == "field")
		{
			s[0].probe_a = 1;
			s[1].probe_a = 1;
			s[1].probe_b = "s";
		}
		if (mode == "w1" || mode == "w2" || mode == "w2n" || mode == "we")
			s[0] thread waits();
		if (mode == "w2")
			s[0] thread waits();
		if (mode == "w2n")
			s[0] thread waits_other();
		if (mode == "we" || mode == "e1")
			s[0] thread ends();
		if (mode == "arr")
		{
			a = [];
			a[0] = 1;
			a[1] = 2;
			s[1].probe_a = a;
			s[2].probe_a = a;
		}
		if (mode == "read")
			x = s[0].probe_q;
		if (mode == "isdef")
			x = isdefined(s[0].probe_q);
		if (mode == "unset")
		{
			s[0].probe_q = 1;
			s[0].probe_q = undefined;
		}
		if (mode == "read2")
		{
			x = s[0].probe_q;
			x = s[0].probe_q;
			x = s[0].probe_r;
		}
	}
	logPrint("PROBE ready\n");
}

waits()
{
	self waittill("never");
}

waits_other()
{
	self waittill("other");
}

ends()
{
	self endon("never");
	wait 1000;
}

calls()
{
	maps\mp\gametypes\probe_usage_ents::pathed();
}

pathed()
{
	ptr = ::other;
	self [[ptr]]();
}

other()
{
	wait 1000;
}
