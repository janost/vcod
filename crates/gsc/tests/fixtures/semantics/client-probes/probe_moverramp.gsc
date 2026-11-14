//	Mover segments to the float's last bit, mp_carentan, no client, for
//	docs/research/cod11-movers.md 24: each phase spawns a script_origin,
//	starts a verb and logs .origin (p) or .angles (a) every frame, each
//	component as its sign, whole part, fraction in 2^-24ths and, when bits
//	remain, the next 24 (the placebits helper). Phases:
//	- ramps on and off frame boundaries, accel only, decel only, accel
//	  straight into decel, a diagonal move, a duration off the frame;
//	- a ramped rotate and rotateVelocity with ramps;
//	- a verb called on a mover mid-segment: rotateVelocity on a running
//	  rotateYaw and on a running rotateVelocity, a moveTo mid accel. The
//	  `before` and `after` rows are read straight before and after the call.
//
//		PROBE p <phase> <step> <time> <x bits> <y bits> <z bits>
//		PROBE a <phase> <step> <time> <pitch bits> <yaw bits> <roll bits>
//
//	Run: tools/run_probe.sh client-probes/probe_moverramp mp_carentan
//	(COD_VERSION=1.5 or uo for the other servers).

main()
{
	thread run();
	maps\mp\gametypes\dm::main();
}

run()
{
	wait 1;
	o = fresh();
	o movez(96, 2, 0.5, 0.5);
	psample("z", o, 2.15);
	o delete();

	o = fresh();
	o movex(100, 1.7, 0.33, 0.27);
	psample("x", o, 1.85);
	o delete();

	o = fresh();
	o moveto(o.origin + (37.3, -12.9, 5.1), 1.3, 0.4, 0.45);
	psample("d", o, 1.45);
	o delete();

	o = fresh();
	o movey(50, 0.333);
	psample("t", o, 0.5);
	o delete();

	o = fresh();
	o movez(-40, 1, 0.5, 0.5);
	psample("c", o, 1.15);
	o delete();

	o = fresh();
	o movex(60, 1.2, 0.35);
	psample("o", o, 1.35);
	o delete();

	o = fresh();
	o movey(-70, 1.15, 0, 0.4);
	psample("e", o, 1.3);
	o delete();

	o = fresh();
	o movez(80, 1.5, 0.6, 0.3);
	psample("k", o, 0.3);
	pshow("k", "before", o);
	o moveto(o.origin + (0, 0, -20), 0.7, 0.2, 0.2);
	pshow("k", "after", o);
	psample("k", o, 0.85);
	o delete();

	r = fresh();
	r rotateyaw(77.7, 1.13, 0.31, 0.29);
	asample("r", r, 1.3);
	r delete();

	r = fresh();
	r rotatevelocity((25, 90, -10), 1.5, 0.3, 0.4);
	asample("v", r, 1.65);
	r delete();

	r = fresh();
	r rotateyaw(90, 1);
	asample("w", r, 0.5);
	ashow("w", "before", r);
	r rotatevelocity((10, 40, 0), 1);
	ashow("w", "after", r);
	asample("w", r, 1.15);
	r delete();

	r = fresh();
	r rotatevelocity((0, 60, 0), 1);
	asample("m", r, 0.4);
	ashow("m", "before", r);
	r rotatevelocity((0, -30, 0), 0.6, 0.2, 0.2);
	ashow("m", "after", r);
	asample("m", r, 0.75);
	r delete();
	logPrint("PROBE done\n");
}

fresh()
{
	e = spawn("script_origin", (-400.25, 2600.5, 40.125));
	wait 0.05;
	return e;
}

pshow(phase, step, e)
{
	o = e.origin;
	logPrint("PROBE p " + phase + " " + step + " " + getTime() + " " + bits(o[0]) + " " + bits(o[1]) + " " + bits(o[2]) + "\n");
}

ashow(phase, step, e)
{
	a = e.angles;
	logPrint("PROBE a " + phase + " " + step + " " + getTime() + " " + bits(a[0]) + " " + bits(a[1]) + " " + bits(a[2]) + "\n");
}

psample(phase, e, secs)
{
	for (t = 0; t < secs; t += 0.05)
	{
		pshow(phase, "f", e);
		wait 0.05;
	}
}

asample(phase, e, secs)
{
	for (t = 0; t < secs; t += 0.05)
	{
		ashow(phase, "f", e);
		wait 0.05;
	}
}

// Sign, whole part, fraction in 2^-24ths, and the next 24 bits when any
// remain: exact for |z| < 65536, built by comparisons since the language has
// no integer cast (probe_placebits).
bits(z)
{
	sign = "+";
	a = z;
	if (z < 0)
	{
		sign = "-";
		a = 0 - z;
	}
	whole = 0;
	p = 32768;
	while (p >= 1)
	{
		if (whole + p <= a)
			whole = whole + p;
		p = p / 2;
	}
	f = a - whole;
	frac = 0;
	for (k = 0; k < 24; k++)
	{
		f = f * 2;
		frac = frac * 2;
		if (f >= 1)
		{
			f = f - 1;
			frac = frac + 1;
		}
	}
	if (f == 0)
		return sign + ":" + whole + ":" + frac;
	rest = 0;
	for (k = 0; k < 24; k++)
	{
		f = f * 2;
		rest = rest * 2;
		if (f >= 1)
		{
			f = f - 1;
			rest = rest + 1;
		}
	}
	return sign + ":" + whole + ":" + frac + ":" + rest;
}
