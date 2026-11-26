//	A static model (`misc_model`) in a locational trace's way
//	(docs/research/cod11-combat.md 2.7.2). Stock tdm, no client needed.
//
//	1 s in, each segment listed for the map is traced with `bulletTrace`
//	(hitting characters). The world alone meets none of them. Each runs
//	into one placement where a clip that walks its collision surfaces one
//	at a time, each culled by its stored bounds short of the best fraction
//	so far, meets another point than a clip of every triangle at once:
//	the surface that would win is walked later and its bounds are entered
//	past the earlier surface's hit, or the triangle that would win lies
//	outside its surface's bounds. The first two of each map are controls,
//	a drop onto a placement both clips agree on.
//
//	PROBE mtrace <time> <segment> <entity number or -1> <fraction> <normal> <position>
//	PROBE mdone <time>

main()
{
	thread traces(getcvar("mapname"));
	maps\mp\gametypes\tdm::main();
}

entnum(e)
{
	if (!isdefined(e))
		return -1;
	return e getEntityNumber();
}

//	Start and end in turn, in the world.
segs(map)
{
	s = [];
	if (map == "mp_depot")
	{
		// controls: the boxcar (placement 54) and the flatcar (49)
		s[s.size] = (-2462, -1437, 222);
		s[s.size] = (-2462, -1437, -24);
		s[s.size] = (-2556, -1737, 222);
		s[s.size] = (-2556, -1737, -24);
		// a later surface's bounds entered past an earlier one's hit
		s[s.size] = (-2571.75, -1483.75, 7.5);
		s[s.size] = (-2560.625, -1486.125, 70.5);
		s[s.size] = (-2485.875, -1781.25, 14.125);
		s[s.size] = (-2487.125, -1782.625, 33);
		s[s.size] = (-370.5, 811.75, 15.25);
		s[s.size] = (-367.5, 847.375, 26.125);
		s[s.size] = (-439.875, 1304.375, 23.625);
		s[s.size] = (-419.5, 1321.75, 24.75);
		s[s.size] = (-619.75, 605, 179.875);
		s[s.size] = (-621, 638.625, 196);
		s[s.size] = (-2094.625, 1580.625, 126);
		s[s.size] = (-2096, 1567, 157.875);
		// the triangle met at once lies outside its surface's bounds
		s[s.size] = (-2084.5, 1543.75, 152.125);
		s[s.size] = (-2096.875, 1569.625, 155.375);
		s[s.size] = (-2089.625, 1276.5, 143.375);
		s[s.size] = (-2097.875, 1265.75, 159.25);
		s[s.size] = (-2650.125, -1416.75, 109.125);
		s[s.size] = (-2647.5, -1449, 134.625);
		s[s.size] = (-618.75, 625.125, 164.875);
		s[s.size] = (-621.125, 638, 199.875);
	}
	else if (map == "mp_chateau")
	{
		// controls: the truck (placement 0) and the staff car (7)
		s[s.size] = (1575, 637, 344);
		s[s.size] = (1575, 637, 148);
		s[s.size] = (-281, 1099, 311);
		s[s.size] = (-281, 1099, 115);
		// a sliver of the truck's cab, under a thousandth of a square unit
		s[s.size] = (1580.125, 592.625, 246.5);
		s[s.size] = (1593.125, 581.25, 196.375);
	}
	return s;
}

traces(map)
{
	wait 1;
	s = segs(map);
	for (i = 0; i + 1 < s.size; i += 2)
	{
		tr = bullettrace(s[i], s[i + 1], true, undefined);
		logPrint("PROBE mtrace " + gettime() + " " + i / 2 + " " + entnum(tr["entity"]) + " " + tr["fraction"] + " " + tr["normal"] + " " + tr["position"] + "\n");
	}
	logPrint("PROBE mdone " + gettime() + "\n");
}
