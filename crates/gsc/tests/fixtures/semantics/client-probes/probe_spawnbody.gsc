//	A spawn inside another player's body (docs/research/cod11-player-clip.md
//	13). The first player to play is the host, the second the spawner; both
//	are `--probe-uo-weapon direct` clients, so the script holds their keys
//	through `iPrintLn("PROBE_IN <buttons> <wbuttons> <up> <forward> <right>")`.
//	Each trial puts the host on mp_carentan's flat brush floor (probe_bump's
//	spot) facing +x, parks the spawner 120 behind it facing -x, sets the
//	spawner's keys 0.4 s ahead (the spawn's think runs the client's last
//	cmd on 1.5 and UO), then `spawn`s him at an offset off the host:
//
//	PROBE host <time> <entity number> <x> <y> <z>
//	PROBE spawned <time> <entity number> <trial> <dx> <yaw> <forward> <x> <y> <z>
//	PROBE pos <time> <entity number> <x> <y> <z>
//	PROBE touch <time> <entity number> <other entity number>
//
//	Origins print exact: each component as its integer part and the
//	remainder. `spawned` is the origin the think left, read by the spawning
//	thread; `pos` is both players every frame for 0.5 s after. Trials, the
//	spawn `dx` along x off the host and its yaw:
//
//	inside_out      8    0  forward 127: in the host, walking out
//	inside_through  8  180  forward 127: in the host, walking through him
//	inside_still    8    0  no keys
//	outside_in    -31    0  forward 127: a unit clear, walking into him
//
//	Then the spawner walks into the host on his own cmds twice: put 60
//	behind him facing +x, forward held 0.6 s, then still:
//
//	PROBE walk <time> <entity number> <start|stop> <x> <y> <z>
//
//	`touch` logs every `"touch"` notify a player gets from another player
//	(`ClientImpacts`).
//
//	`probe_moved 1` then runs `outside_in` twice more each way with the
//	host moved by `setOrigin` in the spawn's frame, ahead of the `spawn`:
//	`moved_in` from 200 ahead onto the spot, `moved_out` off it to 200
//	ahead.

main()
{
	thread watch();
	maps\mp\gametypes\dm::main();
}

spot()
{
	return (1132, -376, -151.875);
}

watch()
{
	if (getcvar("mapname") != "mp_carentan")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	host = undefined;
	spawner = undefined;
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
			else if (p != host && !isdefined(spawner))
				spawner = p;
		}
		if (isdefined(spawner))
			break;
	}
	logPrint("PROBE pair " + getTime() + " " + host getEntityNumber() + " " + spawner getEntityNumber() + "\n");
	host thread log_touches();
	spawner thread log_touches();
	host iprintln("PROBE_IN 0 0 0 0 0");
	wait 2;
	trial(host, spawner, "inside_out", 8, 0, 127);
	trial(host, spawner, "inside_through", 8, 180, 127);
	trial(host, spawner, "inside_still", 8, 0, 0);
	trial(host, spawner, "outside_in", -31, 0, 127);
	trial(host, spawner, "inside_out", 8, 0, 127);
	trial(host, spawner, "inside_through", 8, 180, 127);
	trial(host, spawner, "inside_still", 8, 0, 0);
	trial(host, spawner, "outside_in", -31, 0, 127);
	walk(host, spawner);
	walk(host, spawner);
	if (getcvarint("probe_moved"))
	{
		s = spot();
		moved(host, spawner, "moved_in", s + (200, 0, 0), s);
		moved(host, spawner, "moved_out", s, s + (200, 0, 0));
		moved(host, spawner, "moved_in", s + (200, 0, 0), s);
		moved(host, spawner, "moved_out", s, s + (200, 0, 0));
	}
	logPrint("PROBE done " + getTime() + "\n");
}

//	`probe_moved 1`: `outside_in`'s spawn with the host waiting at `park`
//	and set to `at` in the spawn's own frame, ahead of the `spawn`, so the
//	think meets him where the script just put him or not at all.
moved(host, spawner, name, park, at)
{
	s = spot();
	spawner iprintln("PROBE_IN 0 0 0 0 0");
	host setorigin(park);
	host setplayerangles((0, 0, 0));
	spawner setorigin(s + (-120, 0, 0));
	spawner setplayerangles((0, 180, 0));
	wait 1;
	spawner setorigin(s + (-120, 0, 0));
	spawner iprintln("PROBE_IN 0 0 0 127 0");
	wait 0.4;
	host setorigin(at);
	logPrint("PROBE host " + getTime() + " " + host getEntityNumber() + " " + exact3(host.origin) + "\n");
	spawner spawn(s + (-31, 0, 0), (0, 0, 0));
	logPrint("PROBE spawned " + getTime() + " " + spawner getEntityNumber() + " " + name + " -31 0 127 " + exact3(spawner.origin) + "\n");
	spawner iprintln("PROBE_IN 0 0 0 0 0");
	wait 2;
}

trial(host, spawner, name, dx, yaw, forward)
{
	s = spot();
	spawner iprintln("PROBE_IN 0 0 0 0 0");
	host setorigin(s);
	host setplayerangles((0, 0, 0));
	spawner setorigin(s + (-120, 0, 0));
	spawner setplayerangles((0, 180, 0));
	wait 1;
	logPrint("PROBE host " + getTime() + " " + host getEntityNumber() + " " + exact3(host.origin) + "\n");
	spawner setorigin(s + (-120, 0, 0));
	spawner iprintln("PROBE_IN 0 0 0 " + forward + " 0");
	wait 0.4;
	logPrint("PROBE host " + getTime() + " " + host getEntityNumber() + " " + exact3(host.origin) + "\n");
	spawner spawn(s + (dx, 0, 0), (0, yaw, 0));
	logPrint("PROBE spawned " + getTime() + " " + spawner getEntityNumber() + " " + name + " " + dx + " " + yaw + " " + forward + " " + exact3(spawner.origin) + "\n");
	spawner iprintln("PROBE_IN 0 0 0 0 0");
	for (k = 0; k < 10; k++)
	{
		wait 0.05;
		logPrint("PROBE pos " + getTime() + " " + host getEntityNumber() + " " + exact3(host.origin) + "\n");
		logPrint("PROBE pos " + getTime() + " " + spawner getEntityNumber() + " " + exact3(spawner.origin) + "\n");
	}
	wait 2;
}

walk(host, spawner)
{
	s = spot();
	spawner iprintln("PROBE_IN 0 0 0 0 0");
	host setorigin(s);
	host setplayerangles((0, 0, 0));
	spawner setorigin(s + (-60, 0, 0));
	spawner setplayerangles((0, 0, 0));
	wait 1;
	n = spawner getEntityNumber();
	logPrint("PROBE walk " + getTime() + " " + n + " start " + exact3(spawner.origin) + "\n");
	spawner iprintln("PROBE_IN 0 0 0 127 0");
	wait 0.6;
	spawner iprintln("PROBE_IN 0 0 0 0 0");
	wait 0.5;
	logPrint("PROBE walk " + getTime() + " " + n + " stop " + exact3(spawner.origin) + "\n");
	wait 1;
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
