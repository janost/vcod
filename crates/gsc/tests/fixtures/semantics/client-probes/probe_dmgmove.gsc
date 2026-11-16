//	A player a damage callback moves into another player, and the cmds that
//	run after it (docs/research/cod11-player-clip.md 13.6). The first player
//	to play is the host, the second the faller; both are
//	`--probe-uo-weapon direct` clients, so the script holds their keys
//	through `iPrintLn("PROBE_IN <buttons> <wbuttons> <up> <forward> <right>")`.
//	Each trial stands the host on mp_carentan's flat floor at (60 1600),
//	open to the sky, facing +x, and drops the faller from 300 over a point
//	100 behind it, facing +x, his forward key held or not. His
//	landing's `MOD_FALLING` callback runs inside his own cmd (combat doc 5.5)
//	and moves one of the pair 8 off the other along x, the faller in front:
//
//	self_out     the faller onto the host + (8 0 0), forward 127
//	self_still   the same, no keys
//	other_out    the host onto the faller - (8 0 0), forward 127
//	other_still  the same, no keys
//
//	The callback deals no damage. It logs
//
//	PROBE hurt <time> <entity number> <trial> <damage> <faller x y z> <host x y z>
//	PROBE moved <time> <entity number> <x y z>
//
//	the origins it read and the moved player's after the `setOrigin`. From
//	the drop on, both players' origins every frame for 1.5 s, the faller's
//	keys let go after 1 s:
//
//	PROBE drop <time> <entity number> <trial> <forward>
//	PROBE pos <time> <entity number> <x y z>
//	PROBE touch <time> <entity number> <other entity number>
//
//	Origins print exact: each component as its integer part and the
//	remainder. `touch` logs every `"touch"` notify a player gets from
//	another player (`ClientImpacts`).

main()
{
	thread watch();
	maps\mp\gametypes\dm::main();
}

spot()
{
	return (60, 1600, 0.125);
}

watch()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	wait 0.05;
	level.callbackPlayerDamage = ::on_damage;
	host = undefined;
	faller = undefined;
	for (;;)
	{
		wait 0.05;
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (!isdefined(p.sessionstate) || p.sessionstate != "playing")
				continue;
			if (!isdefined(host))
				host = p;
			else if (p != host && !isdefined(faller))
				faller = p;
		}
		if (isdefined(faller))
			break;
	}
	level.host = host;
	level.faller = faller;
	logPrint("PROBE pair " + getTime() + " " + host getEntityNumber() + " " + faller getEntityNumber() + "\n");
	host thread log_touches();
	faller thread log_touches();
	host iprintln("PROBE_IN 0 0 0 0 0");
	wait 2;
	trial(host, faller, "self_out", 127);
	trial(host, faller, "self_still", 0);
	trial(host, faller, "other_out", 127);
	trial(host, faller, "other_still", 0);
	trial(host, faller, "self_out", 127);
	trial(host, faller, "self_still", 0);
	trial(host, faller, "other_out", 127);
	trial(host, faller, "other_still", 0);
	logPrint("PROBE done " + getTime() + "\n");
}

trial(host, faller, name, forward)
{
	s = spot();
	faller iprintln("PROBE_IN 0 0 0 0 0");
	host setorigin(s);
	host setplayerangles((0, 0, 0));
	faller setorigin(s + (-100, 0, 0));
	faller setplayerangles((0, 0, 0));
	wait 1;
	faller iprintln("PROBE_IN 0 0 0 " + forward + " 0");
	wait 0.4;
	level.trial = name;
	faller setorigin(s + (-100, 0, 300));
	logPrint("PROBE drop " + getTime() + " " + faller getEntityNumber() + " " + name + " " + forward + "\n");
	for (k = 0; k < 30; k++)
	{
		wait 0.05;
		if (k == 20)
			faller iprintln("PROBE_IN 0 0 0 0 0");
		logPrint("PROBE pos " + getTime() + " " + host getEntityNumber() + " " + exact3(host.origin) + "\n");
		logPrint("PROBE pos " + getTime() + " " + faller getEntityNumber() + " " + exact3(faller.origin) + "\n");
	}
	level.trial = undefined;
	wait 2;
}

//	The faller's landing, inside his cmd: the trial's move, once.
on_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	if (!isdefined(level.trial) || self != level.faller || sMeansOfDeath != "MOD_FALLING")
		return;
	name = level.trial;
	level.trial = undefined;
	host = level.host;
	logPrint("PROBE hurt " + getTime() + " " + self getEntityNumber() + " " + name + " " + iDamage + " " + exact3(self.origin) + " " + exact3(host.origin) + "\n");
	if (name == "self_out" || name == "self_still")
	{
		self setorigin(host.origin + (8, 0, 0));
		moved = self;
	}
	else
	{
		host setorigin(self.origin - (8, 0, 0));
		moved = host;
	}
	logPrint("PROBE moved " + getTime() + " " + moved getEntityNumber() + " " + exact3(moved.origin) + "\n");
}

log_touches()
{
	self endon("disconnect");
	n = self getEntityNumber();
	for (;;)
	{
		self waittill("touch", other);
		if (isdefined(other) && isplayer(other))
			logPrint("PROBE touch " + getTime() + " " + n + " " + other getEntityNumber() + "\n");
	}
}

//	probe_pronebones' exact float: the integer part and the remainder.
exact(f)
{
	setcvar("probe_int", "" + f);
	i = getcvarint("probe_int");
	return i + " " + (f - i);
}

exact3(v)
{
	return exact(v[0]) + " " + exact(v[1]) + " " + exact(v[2]);
}
