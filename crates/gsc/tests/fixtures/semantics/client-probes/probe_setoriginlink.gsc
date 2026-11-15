//	Which origin a `setOrigin` links a player at (docs/research/
//	cod11-combat.md 14.1, 14.3): `radiusDamage`'s second chance keeps a
//	victim every probe of `CanDamage` missed only when the midpoint of his
//	linked box, `0.5 * (absmin + absmax)`, is nearer the blast than
//	`radius * 0.2`, and deals `(int)(points * 0.1)`. Stock tdm on mp_carentan
//	under tools/run_probe.sh, one `--probe-uo-weapon direct` axis client.
//
//	The script puts him in the air under the sky brush over the garden, at
//	an origin with a fraction on every axis, and sets off blasts over the
//	sky brush, so the brush blocks every probe and the midpoint trace. A
//	sweep of the radius in one frame finds where the second chance starts
//	to land: five times the midpoint's distance, read off the box the link
//	left. `set` sweeps in the frame of the `setOrigin`, `cmd` two frames
//	later, once his cmds have relinked him where he fell. Each sweep logs
//	each radius where the callback's damage changed, the first included,
//	and the verdict names the origin whose midpoint the first landing radius
//	sits within four steps past five times the distance of (`unsnapped`,
//	`snapped` for the truncated one, `both`, `neither`, or `none` landed):
//
//	PROBE sky <under the brush> <over it>
//	PROBE case set <origin> <blast>
//	PROBE edge <name> <radius> <damage>
//	PROBE fell <origin>
//	PROBE verdict <set|cmd> <which>

main()
{
	thread run();
	maps\mp\gametypes\tdm::main();
	level.callbackPlayerDamage = ::hit;
}

hit(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	level.probe_hit = iDamage;
}

//	The radius sweep over the whole numbers [lo, hi] in steps of 1/64: each
//	radius where the callback's damage changed, the first included.
sweep(name, blast, lo, hi)
{
	last = -1;
	first = undefined;
	for (i = lo * 64; i <= hi * 64; i++)
	{
		r = i * 0.015625;
		level.probe_hit = 0;
		radiusDamage(blast, r, 100, 100);
		if (level.probe_hit != last)
			logPrint("PROBE edge " + name + " " + fine1(r) + " " + level.probe_hit + "\n");
		if (level.probe_hit > 0 && !isdefined(first))
			first = r;
		last = level.probe_hit;
	}
	return first;
}

//	Which link the second chance read: the first radius that landed sits
//	within four steps past five times the midpoint's distance off the linked
//	origin, unsnapped or truncated (14.3).
verdict(name, blast, o)
{
	mid = (0, 0, 35);
	du = 5 * distance(blast, o + mid);
	ds = 5 * distance(blast, (trunc(o[0]), trunc(o[1]), trunc(o[2])) + mid);
	lo = du;
	if (ds < lo)
		lo = ds;
	hi = du;
	if (ds > hi)
		hi = ds;
	first = sweep(name, blast, trunc(lo) - 1, trunc(hi) + 2);
	v = "neither";
	if (!isdefined(first))
		v = "none";
	else if (first - du > 0 && first - du <= 0.0625 && first - ds > 0 && first - ds <= 0.0625)
		v = "both";
	else if (first - du > 0 && first - du <= 0.0625)
		v = "unsnapped";
	else if (first - ds > 0 && first - ds <= 0.0625)
		v = "snapped";
	logPrint("PROBE verdict " + name + " " + v + "\n");
}

trunc(x)
{
	if (x < 0)
		return 0 - whole(0 - x);
	return whole(x);
}

run()
{
	wait 1;
	spot = (932, -376, -151.875);
	under = bullettrace(spot + (0, 0, 8), spot + (0, 0, 8000), false, undefined)["position"];
	over = bullettrace(under + (0, 0, 4000), under, false, undefined)["position"];
	logPrint("PROBE sky " + fine(under) + " " + fine(over) + "\n");
	for (;;)
	{
		pl = getentarray("player", "classname");
		if (pl.size > 0 && pl[0].sessionstate == "playing")
			break;
		wait 0.05;
	}
	p = pl[0];
	wait 2;
	o = (932.75, -376.75, under[2] - 80.625);
	blast = (972, -376, over[2] + 4);
	p setorigin(o);
	logPrint("PROBE case set " + fine(p.origin) + " " + fine(blast) + "\n");
	verdict("set", blast, p.origin);
	//	Two frames on he has fallen a few units on his own cmds, whose clock
	//	sets where: his origin is logged apart (`PROBE fell`), and only the
	//	verdict is replayed.
	wait 0.1;
	logPrint("PROBE fell " + fine(p.origin) + "\n");
	verdict("cmd", blast, p.origin);
	logPrint("PROBE done\n");
}

fine(v)
{
	return fine1(v[0]) + "," + fine1(v[1]) + "," + fine1(v[2]);
}

fine1(x)
{
	sign = "";
	if (x < 0)
	{
		sign = "-";
		x = 0 - x;
	}
	n = whole(x);
	r = (x - n) * 1000;
	m = whole(r);
	return sign + n + ":" + m + ":" + ((r - m) * 1000000);
}

//	The engine has no int(): whole() takes the whole part by halving steps.
whole(x)
{
	n = 0;
	for (b = 8192; b >= 1; b = b / 2)
	{
		if (x - n >= b)
			n = n + b;
	}
	return n;
}
