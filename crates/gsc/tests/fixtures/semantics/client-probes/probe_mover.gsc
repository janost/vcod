//	The ten scriptent mover verbs, sampled once per server frame: what unit the
//	time argument is in, what accel and decel do to the curve, and when the
//	completion notify fires.
//	Run by tools/run_probe.sh; every logPrint line is one measurement. No
//	client is needed, but a --net-probe against the same server while it runs
//	is what answers the other half, which is what the moving entity puts on
//	the wire.
//	Groups are separate files because a script runtime error takes the whole
//	retail server down, so one fatal expression would cost every measurement
//	after it. Nothing here blocks on a notify for that reason: a `waittill`
//	on an event the engine does not raise would hang the thread and cost
//	every phase after it, so the completion notifies are watched by their own
//	threads and the phases are sequenced by plain waits.

main()
{
	thread measure();
	thread measure_wire();

	//	The real gametype, so the map loads and the server stays up.
	maps\mp\gametypes\dm::main();
}

measure()
{
	//	Past the whole bootstrap, the way the trigger probe waits.
	wait 1;
	logPrint("PROBE time0 " + getTime() + "\n");

	//	Two durations of the same distance: if the argument is seconds the
	//	second takes twice as long, and if it is frames or milliseconds
	//	neither takes any time at all.
	logPrint("PROBE at moveto_t1\n");
	phase_moveto("moveto_t1", 1);

	logPrint("PROBE at moveto_t2\n");
	phase_moveto("moveto_t2", 2);

	//	The same move with a ramp on each end. Whatever accel and decel mean
	//	numerically, the difference between this curve and moveto_t2's is it.
	logPrint("PROBE at moveto_ramp\n");
	phase_moveto_ramp("moveto_ramp", 2, 0.5, 0.5);

	//	Is the axis verb's argument a delta or an absolute coordinate? The
	//	entity starts at x = 100, so 500 tells the two apart.
	logPrint("PROBE at movex\n");
	phase_movex("movex", 500, 2);

	//	Accel with no decel, to see whether the two ends are independent.
	logPrint("PROBE at moveto_accel\n");
	phase_moveto_ramp("moveto_accel", 2, 0.5, 0);

	logPrint("PROBE at rotateyaw\n");
	phase_rotateyaw("rotateyaw", 90, 2);

	//	The same verb twice on one entity: a delta lands at 180, an absolute
	//	target at 90. `movex` is already settled -- 500 from x = 100 landed at
	//	600 -- but the rotate verbs start from zero, where the two agree.
	logPrint("PROBE at rotateyaw_twice\n");
	phase_rotateyaw_twice("rotateyaw_twice", 90, 1);

	logPrint("PROBE at rotateto\n");
	phase_rotateto("rotateto", (0, 90, 0), 2, 0.5, 0.5);

	//	The corpus's own call shape, mp_pavlov's closure included.
	logPrint("PROBE at rotatevelocity\n");
	phase_rotatevelocity("rotatevelocity", (0, 180, 0), 2, 0, 0);

	logPrint("PROBE at movegravity\n");
	phase_movegravity("movegravity", (0, 0, 300), 3);

	logPrint("PROBE end\n");
}

//	The wire half. A spawned `script_origin` carries no model and reaches no
//	client, so what a mover puts on the wire has to be measured on entities a
//	client is actually sent: the map's own `script_model`s. They ride up and
//	down for the whole run, concurrently with the phases above and on their own
//	entities, so a `--net-probe` attached at any point in the run finds one in
//	motion. Nothing is sampled per frame here; the client capture is the
//	measurement and this only says which entity numbers to read.
measure_wire()
{
	wait 1;

	//	The mover verbs refuse anything else: a `movez` on a placed weapon is
	//	the fatal `entity N is not a script_brushmodel, script_model, or
	//	script_origin`, which cost this probe one run to learn.
	ents = getentarray("script_model", "classname");
	ents = add_all(ents, getentarray("script_brushmodel", "classname"));

	logPrint("PROBE wire_count " + ents.size + "\n");
	for (i = 0; i < ents.size; i++)
		logPrint("PROBE wire_ent " + ents[i] getEntityNumber() + " " + ents[i].classname + " " + ents[i] getorigin() + "\n");

	//	mp_pavlov's are all in two corners and a lone client spawns anywhere,
	//	so a copy is also parked over each player as it appears.
	thread follow_players(ents[0].model);

	for (;;)
	{
		logPrint("PROBE wire_up " + getTime() + "\n");
		for (i = 0; i < ents.size; i++)
			ents[i] movez(96, 2);
		wait 3;

		logPrint("PROBE wire_down " + getTime() + "\n");
		for (i = 0; i < ents.size; i++)
			ents[i] movez(-96, 2);
		wait 3;
	}
}

//	One bobbing script_model per player, spawned where that player stands, so
//	the moving entity is in its PVS whatever the spawn picked. The model is
//	borrowed off a map entity because it is already precached; a name the map
//	does not use would need a precache the gametype cannot do at this point.
follow_players(modelname)
{
	logPrint("PROBE wire_follow_model " + modelname + "\n");
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (isDefined(players[i].probe_mover))
				continue;
			e = spawn("script_model", players[i] getorigin() + (0, 0, 64));
			e setmodel(modelname);
			players[i].probe_mover = e;
			logPrint("PROBE wire_follow " + e getEntityNumber() + " over " + players[i] getEntityNumber() + " at " + e getorigin() + "\n");
			e thread bob();
			thread throwaways(e getorigin(), modelname);
		}
		wait 1;
	}
}

//	A cycle covering every wire shape a mover has: a plain move, a ramped one
//	(the trapezoid `moveto_ramp` measured, which no single linear trajectory
//	can carry), and a rotate, which is the only thing that writes `apos`.
bob()
{
	for (;;)
	{
		self movez(96, 2);
		wait 3;
		self movez(-96, 2);
		wait 3;
		self movez(96, 2, 0.5, 0.5);
		wait 3;
		self movez(-96, 2, 0.5, 0.5);
		wait 3;
		self rotateyaw(90, 2);
		wait 3;
	}
}

//	The two verbs that leave the entity somewhere it cannot come back from --
//	`movegravity` falls out of the map and `rotatevelocity` ends on whatever
//	angle the duration lands on -- get a fresh entity each round and are
//	deleted after.
throwaways(org, modelname)
{
	for (;;)
	{
		e = spawn("script_model", org);
		e setmodel(modelname);
		logPrint("PROBE wire_gravity " + e getEntityNumber() + " " + getTime() + "\n");
		e movegravity((0, 0, 200), 2);
		wait 3;
		e delete();

		e = spawn("script_model", org);
		e setmodel(modelname);
		logPrint("PROBE wire_spin " + e getEntityNumber() + " " + getTime() + "\n");
		e rotatevelocity((0, 180, 0), 2, 0, 0);
		wait 3;
		e delete();
	}
}

//	Appends `b` to `a`. gsc has no array concatenation.
add_all(a, b)
{
	for (i = 0; i < b.size; i++)
		a[a.size] = b[i];
	return a;
}

//	One entity per phase, so no phase starts from where the last one stopped.
new_mover(tag)
{
	e = spawn("script_origin", (100, 0, 100));
	e watch(tag);
	return e;
}

//	The per-frame sampler and both completion watchers, on self.
watch(tag)
{
	self thread sample(tag, 60);
	self thread done(tag, "movedone");
	self thread done(tag, "rotatedone");
}

sample(tag, frames)
{
	self endon("probe_stop");
	for (i = 0; i < frames; i++)
	{
		logPrint("PROBE p " + tag + " " + getTime() + " " + self getorigin() + " " + self angles_or_undef() + "\n");
		wait 0.05;
	}
}

done(tag, event)
{
	self endon("probe_stop");
	self waittill(event);
	logPrint("PROBE done " + tag + " " + event + " " + getTime() + " " + self getorigin() + " " + self angles_or_undef() + "\n");
}

//	Whether the engine writes `.angles` back while a rotate runs is itself one
//	of the measurements, so an unwritten field is recorded rather than
//	pre-set: concatenating an undefined value is fatal.
angles_or_undef()
{
	if (isDefined(self.angles))
		return "" + self.angles;
	return "undef";
}

//	Every phase ends the same way: let the sampler run out, then stop the
//	watchers and drop the entity.
end_phase(e)
{
	wait 3.1;
	e notify("probe_stop");
	e delete();
}

phase_moveto(tag, seconds)
{
	e = new_mover(tag);
	e moveto((600, 0, 100), seconds);
	end_phase(e);
}

phase_moveto_ramp(tag, seconds, accel, decel)
{
	e = new_mover(tag);
	e moveto((600, 0, 100), seconds, accel, decel);
	end_phase(e);
}

phase_movex(tag, x, seconds)
{
	e = new_mover(tag);
	e movex(x, seconds);
	end_phase(e);
}

phase_rotateyaw(tag, yaw, seconds)
{
	e = new_mover(tag);
	e rotateyaw(yaw, seconds);
	end_phase(e);
}

phase_rotateyaw_twice(tag, yaw, seconds)
{
	e = new_mover(tag);
	e rotateyaw(yaw, seconds);
	wait 1.5;
	e rotateyaw(yaw, seconds);
	end_phase(e);
}

phase_rotateto(tag, angles, seconds, accel, decel)
{
	e = new_mover(tag);
	e rotateto(angles, seconds, accel, decel);
	end_phase(e);
}

phase_rotatevelocity(tag, v, seconds, accel, decel)
{
	e = new_mover(tag);
	e rotatevelocity(v, seconds, accel, decel);
	end_phase(e);
}

phase_movegravity(tag, v, seconds)
{
	e = new_mover(tag);
	e movegravity(v, seconds);
	end_phase(e);
}
