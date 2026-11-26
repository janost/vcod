//	A script model in the way of a locational trace and of rounds
//	(docs/research/cod11-combat.md 2.7). Stock tdm.
//
//	The trace part, any patch, no client needed: 2 s in a `script_model`
//	with `xmodel/barrel_black1` (a 44-unit collision cylinder) is spawned
//	64 units ahead of the first team deathmatch spawn point, and a
//	`bulletTrace` from 24 above the spawn point through the barrel's middle
//	is logged with the barrel shown, hidden, shown and `notSolid()`, then
//	deleted:
//
//	PROBE model <entity number> <origin>
//	PROBE mtrace <time> <phase> <entity number or -1> <fraction> <surfacetype> <position>
//
//	The rounds part, United Offensive only (it needs a vehicle to take the
//	rounds that get past): client 0, the shooter (`--probe-uo-weapon aim`),
//	is put 250 units off the right flank of the first `willyjeep_mp` with
//	its view turned every frame onto the point 20 above the jeep's origin,
//	and a second barrel stands three quarters of the way there on the
//	ground under that line. The barrel is shown for 3 s, hidden for 3 s and
//	deleted; each phase's start is logged, and every `damage` notify the
//	jeep takes:
//
//	PROBE place <time> <entity number> <origin> <yaw>
//	PROBE rphase <time> <phase>
//	PROBE vdamage <time> <amount> <mod> <point>
//
//	The pair parts, any patch, `+set probe_part rounds` or `grenade`: an
//	allies shooter or thrower and an axis target, both held where the
//	script puts them every frame. The site is the first tdm spawn point
//	past the first with 256 units of level, clear ground on its facing:
//	the allies player stands on it, the axis player 256 ahead facing back.
//	Once both are in, the target's model and attachments (the gear the dm
//	script drew):
//
//	PROBE body <time> <model> [ <tag>:<attached model>]...
//
//	rounds: the shooter (`--probe-uo-weapon aim`, a Thompson) is turned
//	every frame onto the target's waist, 30 above its feet, and a barrel
//	stands 230 units out (26 in front of the target) for 3 s, hidden 3 s,
//	then deleted; the target is a plain `--probe-team axis` probe.
//
//	grenade: the thrower (`--probe-uo-weapon throw`, which pulls back for a
//	second and lets go twice, 6 s apart) is handed the frag and turned onto
//	the middle of a barrel 64 units out, the target 160 out behind it. The
//	barrel is deleted 5 s after the first grenade leaves, before the second.
//	Each live grenade's origin is logged every frame. Under +set
//	probe_contents <n> (UO) the barrel takes setContents(n) when it is
//	spawned, logged as PROBE contents <time> <n>.
//
//	blast: the grenade part's thrower and aim with nothing in the way; the
//	target waits 48 behind the thrower until a grenade comes to rest, then
//	stands 96 back from it toward the thrower until it goes off. A barrel is
//	spawned 32 back from the first grenade where it rests (between it and
//	the target), deleted 5 s after the throw; the second goes off in the
//	open:
//
//	PROBE rest <time> <entity number> <origin>
//
//	prone: the grenade part's thrower with no barrel, the target (a
//	`--probe-fall --probe-fall-prone <site yaw + 180>` probe) lying 96 out
//	facing back, the aim on his back, 20 past and 8 above his origin
//	(his body lies behind his origin, away from the thrower), held there
//	across the ground only. Once the first
//	grenade rests the target is moved 96 behind the thrower, so the second
//	lands where he lay. Each rest is logged as in the blast part; the
//	missiles' `groundEntityNum` rides the thrower's `VEH missile` lines.
//	The thrower's origin is logged, a float per axis, in the frame each
//	grenade first shows:
//
//	PROBE shooter <time> <entity number> <x> <y> <z>
//
//	and every frame of his first second in the pair, before that frame's
//	`setOrigin`, with his offset from the site in thousandths of a unit:
//
//	PROBE thrower <time> <x> <y> <z> <dx> <dy> <dz>
//
//	PROBE site <time> <allies spot> <axis spot> <yaw>
//	PROBE rphase <time> <phase> [<origin>]
//	PROBE gren <time> <entity number> <origin>
//
//	A player's damage callback only logs, by team, and takes no health:
//
//	PROBE hit <time> <team> dmg <amount> mod <mod> loc <location>
//
//	surfs, any patch, no client needed: `+set probe_model <xmodel>`, one
//	surf_segs lists, is spawned as a `script_model` 300 over the first tdm
//	spawn point with 600 units of open sky above it, at yaw 0 and then at
//	yaw 90, and each listed segment, given in the model's frame, is traced
//	with `bulletTrace` (hitting characters) against it; then once more with
//	the model deleted, the world alone. Each segment ends just past two of
//	the model's collision surfaces where they meet, so a clip that walks
//	the surfaces one at a time, each culled by its stored bounds, meets
//	another than one that tries every triangle (combat doc 2.7.1):
//
//	PROBE smodel <time> <phase> <entity number> <origin>
//	PROBE strace <time> <phase> <segment> <entity number or -1> <fraction> <normal> <position>

main()
{
	precacheModel("xmodel/barrel_black1");
	precacheItem("fraggrenade_mp");
	part = getcvar("probe_part");
	if (part == "surfs")
	{
		precacheModel(getcvar("probe_model"));
		thread surfs_part(getcvar("probe_model"));
	}
	else if (part == "")
	{
		thread trace_part();
		thread watch_places();
	}
	else
		thread pair_part(part);
	maps\mp\gametypes\tdm::main();
	level.callbackPlayerDamage = ::probe_damage;
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	who = self getEntityNumber();
	if (isdefined(self.pers["team"]))
		who = self.pers["team"];
	logPrint("PROBE hit " + gettime() + " " + who + " dmg " + iDamage + " mod " + sMeansOfDeath + " loc " + sHitLoc + "\n");
}

entnum(e)
{
	if (!isdefined(e))
		return -1;
	return e getEntityNumber();
}

mtrace(phase, eye, at)
{
	d = vectornormalize(at - eye);
	trace = bullettrace(eye, at + (d[0] * 200, d[1] * 200, d[2] * 200), true, undefined);
	logPrint("PROBE mtrace " + gettime() + " " + phase + " " + entnum(trace["entity"]) + " " + trace["fraction"] + " " + trace["surfacetype"] + " " + trace["position"] + "\n");
}

trace_part()
{
	wait 2;
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	if (!spawns.size)
		spawns = getentarray("mp_tdm_spawn", "classname");
	sp = spawns[0];
	fwd = anglestoforward((0, sp.angles[1], 0));
	ground = bullettrace(sp.origin + (0, 0, 16), sp.origin + (0, 0, -64), false, undefined);
	base = ground["position"];
	at = base + (fwd[0] * 64, fwd[1] * 64, 20);
	model = spawn("script_model", base + (fwd[0] * 64, fwd[1] * 64, 0));
	model setmodel("xmodel/barrel_black1");
	logPrint("PROBE model " + model getEntityNumber() + " " + model.origin + "\n");
	eye = base + (0, 0, 24);
	wait 0.1;
	mtrace("shown", eye, at);
	model hide();
	wait 0.1;
	mtrace("hidden", eye, at);
	model show();
	model notsolid();
	wait 0.1;
	mtrace("notsolid", eye, at);
	model delete();
	wait 0.1;
	mtrace("deleted", eye, at);
}

//	The first `willyjeep_mp` by entity number.
probe_vehicle()
{
	vehs = getentarray("script_vehicle", "classname");
	best = undefined;
	for (i = 0; i < vehs.size; i++)
	{
		if (vehs[i].vehicletype != "willyjeep_mp")
			continue;
		if (!isdefined(best) || vehs[i] getEntityNumber() < best getEntityNumber())
			best = vehs[i];
	}
	return best;
}

log_vehicle_damage()
{
	for (;;)
	{
		self waittill("damage", amount, attacker, dir, point, mod);
		logPrint("PROBE vdamage " + gettime() + " " + amount + " " + mod + " " + point + "\n");
	}
}

watch_places()
{
	wait 2;
	veh = probe_vehicle();
	if (!isdefined(veh))
		return;
	veh thread log_vehicle_damage();
	model = undefined;
	placed = undefined;
	for (;;)
	{
		players = getentarray("player", "classname");
		shooter = undefined;
		for (i = 0; i < players.size; i++)
		{
			players[i] try_place(veh);
			if (isdefined(players[i].probe_placed) && players[i] getEntityNumber() == 0)
				shooter = players[i];
		}
		if (isdefined(shooter))
		{
			eye = shooter.origin + (0, 0, 60);
			at = veh.origin + (0, 0, 20);
			if (!isdefined(placed))
			{
				placed = gettime();
				d = at - eye;
				x = eye + (d[0] * 0.75, d[1] * 0.75, d[2] * 0.75);
				ground = bullettrace(x + (0, 0, 64), x + (0, 0, -128), false, undefined);
				model = spawn("script_model", ground["position"]);
				model setmodel("xmodel/barrel_black1");
				logPrint("PROBE rphase " + gettime() + " shown " + model.origin + "\n");
			}
			else if (gettime() == placed + 3000)
			{
				model hide();
				logPrint("PROBE rphase " + gettime() + " hidden\n");
			}
			else if (gettime() == placed + 6000)
			{
				model delete();
				logPrint("PROBE rphase " + gettime() + " deleted\n");
			}
			shooter setWeaponSlotClipAmmo("primary", 20);
			shooter giveMaxAmmo(shooter getcurrentweapon());
			shooter setplayerangles(vectortoangles(at - eye));
		}
		wait 0.05;
	}
}

//	Early returns: sessionstate is undefined before the first spawn.
try_place(veh)
{
	if (!isdefined(self.sessionstate))
		return;
	if (self.sessionstate != "playing")
	{
		self.probe_placed = undefined;
		return;
	}
	if (isdefined(self.probe_placed))
		return;
	self.probe_placed = 1;
	right = anglestoright(veh.angles);
	spot = veh.origin + (right[0] * 250, right[1] * 250, 8);
	yaw = vectortoangles(veh.origin - spot)[1];
	self setorigin(spot);
	self setplayerangles((10, yaw, 0));
	logPrint("PROBE place " + gettime() + " " + self getEntityNumber() + " " + spot + " " + yaw + "\n");
}

ground_at(p)
{
	tr = bullettrace(p + (0, 0, 64), p + (0, 0, -128), false, undefined);
	return tr["position"];
}

//	The first spawn point past the first whose facing has 256 units of
//	level ground clear at 20 and 60 above it.
pick_site()
{
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	if (!spawns.size)
		spawns = getentarray("mp_tdm_spawn", "classname");
	for (i = 1; i < spawns.size; i++)
	{
		yaw = spawns[i].angles[1];
		fwd = anglestoforward((0, yaw, 0));
		a = ground_at(spawns[i].origin + (0, 0, 8));
		b = ground_at(a + (fwd[0] * 256, fwd[1] * 256, 0));
		dz = b[2] - a[2];
		if (dz > 2 || dz < -2)
			continue;
		if (bullettrace(a + (0, 0, 60), b + (0, 0, 60), false, undefined)["fraction"] < 1)
			continue;
		if (bullettrace(a + (0, 0, 20), b + (0, 0, 20), false, undefined)["fraction"] < 1)
			continue;
		mid = ground_at(a + (fwd[0] * 128, fwd[1] * 128, 0));
		dz = mid[2] - a[2];
		if (dz > 2 || dz < -2)
			continue;
		site = spawnstruct();
		site.a = a + (0, 0, 0.125);
		site.b = b + (0, 0, 0.125);
		site.yaw = yaw;
		site.fwd = fwd;
		return site;
	}
	return undefined;
}

team_player(team)
{
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		if (isdefined(p.sessionstate) && p.sessionstate == "playing" && isdefined(p.pers["team"]) && p.pers["team"] == team)
			return p;
	}
	return undefined;
}

log_grenades()
{
	gs = getentarray("grenade", "classname");
	for (i = 0; i < gs.size; i++)
		logPrint("PROBE gren " + gettime() + " " + gs[i] getEntityNumber() + " " + gs[i].origin + "\n");
	return gs.size;
}

pair_part(part)
{
	wait 1;
	site = pick_site();
	if (!isdefined(site))
	{
		logPrint("PROBE site none\n");
		return;
	}
	logPrint("PROBE site " + gettime() + " " + site.a + " " + site.b + " " + site.yaw + "\n");
	fwd = site.fwd;
	throwing = part == "grenade" || part == "blast" || part == "prone";
	if (part == "prone")
	{
		tpos = site.a + (fwd[0] * 96, fwd[1] * 96, 0);
		at = tpos + (fwd[0] * 20, fwd[1] * 20, 8);
	}
	else if (throwing)
	{
		bpos = site.a + (fwd[0] * 64, fwd[1] * 64, 0);
		tpos = site.a + (fwd[0] * 160, fwd[1] * 160, 0);
		at = bpos + (0, 0, 20);
	}
	else
	{
		bpos = site.a + (fwd[0] * 230, fwd[1] * 230, 0);
		tpos = site.b;
		at = site.b + (0, 0, 30);
	}
	if (part == "blast")
		tpos = site.a - (fwd[0] * 48, fwd[1] * 48, 0);
	parked = tpos;
	model = undefined;
	placed = undefined;
	armed = false;
	thrown = undefined;
	last = undefined;
	rested = 0;
	shot = 0;
	for (;;)
	{
		shooter = team_player("allies");
		target = team_player("axis");
		if (isdefined(shooter) && isdefined(target))
		{
			now = gettime();
			if (!isdefined(placed))
			{
				placed = now;
				logPrint("PROBE body " + now + " " + target.model + attached(target) + "\n");
				if (part == "grenade" || part == "rounds")
				{
					model = spawn("script_model", ground_at(bpos));
					model setmodel("xmodel/barrel_black1");
					logPrint("PROBE rphase " + now + " shown " + model.origin + "\n");
					if (getcvar("probe_contents") != "")
					{
						model setcontents(getcvarint("probe_contents"));
						logPrint("PROBE contents " + now + " " + getcvarint("probe_contents") + "\n");
					}
				}
			}
			if (part == "prone" && now < placed + 1000)
			{
				o = shooter.origin;
				d = o - site.a;
				logPrint("PROBE thrower " + now + " " + o[0] + " " + o[1] + " " + o[2] + " " + d[0] * 1000 + " " + d[1] * 1000 + " " + d[2] * 1000 + "\n");
			}
			if (part == "prone")
			{
				gs = getentarray("grenade", "classname");
				if (gs.size && gs[gs.size - 1] getEntityNumber() != shot)
				{
					shot = gs[gs.size - 1] getEntityNumber();
					o = shooter.origin;
					logPrint("PROBE shooter " + now + " " + shot + " " + o[0] + " " + o[1] + " " + o[2] + "\n");
				}
				if (!gs.size)
					last = undefined;
				else if (isdefined(last) && gs[0].origin == last && rested != gs[0] getEntityNumber())
				{
					rested = gs[0] getEntityNumber();
					logPrint("PROBE rest " + now + " " + rested + " " + gs[0].origin + "\n");
					tpos = ground_at(site.a - (fwd[0] * 96, fwd[1] * 96, 0)) + (0, 0, 0.125);
				}
				else if (gs.size)
					last = gs[0].origin;
			}
			if (part == "blast")
			{
				gs = getentarray("grenade", "classname");
				if (!gs.size)
				{
					tpos = parked;
					last = undefined;
				}
				else if (isdefined(last) && gs[0].origin == last && rested < gs[0] getEntityNumber())
				{
					rested = gs[0] getEntityNumber();
					rest = gs[0].origin;
					tpos = ground_at(rest - (fwd[0] * 96, fwd[1] * 96, 0)) + (0, 0, 0.125);
					logPrint("PROBE rest " + now + " " + rested + " " + rest + "\n");
					if (!isdefined(thrown) || now < thrown + 5000)
					{
						model = spawn("script_model", ground_at(rest - (fwd[0] * 32, fwd[1] * 32, 0)));
						model setmodel("xmodel/barrel_black1");
						logPrint("PROBE rphase " + now + " shown " + model.origin + "\n");
					}
				}
				else if (gs.size)
					last = gs[0].origin;
			}
			if (throwing)
			{
				if (!armed && now >= placed + 1000)
				{
					armed = true;
					shooter setWeaponSlotWeapon("grenade", "fraggrenade_mp");
					shooter giveMaxAmmo("fraggrenade_mp");
					shooter switchToWeapon("fraggrenade_mp");
				}
				if (log_grenades() > 0 && !isdefined(thrown))
					thrown = now;
				if (isdefined(thrown) && isdefined(model) && now == thrown + 5000)
				{
					model delete();
					logPrint("PROBE rphase " + now + " deleted\n");
				}
			}
			else
			{
				if (now == placed + 3000)
				{
					model hide();
					logPrint("PROBE rphase " + now + " hidden\n");
				}
				else if (now == placed + 6000)
				{
					model delete();
					logPrint("PROBE rphase " + now + " deleted\n");
				}
				shooter setWeaponSlotClipAmmo("primary", 20);
				shooter giveMaxAmmo(shooter getcurrentweapon());
			}
			if (distance(shooter.origin, site.a) > 0.5)
				shooter setorigin(site.a);
			//	The prone target is held across the ground only: lying on a
			//	grade he settles more than half a unit under tpos, and a
			//	setorigin every frame would drop him onto it every frame.
			off = target.origin - tpos;
			if (part == "prone")
				off = (off[0], off[1], 0);
			if (length(off) > 0.5)
				target setorigin(tpos);
			eye = site.a + (0, 0, 60);
			shooter setplayerangles(vectortoangles(at - eye));
			target setplayerangles((0, site.yaw + 180, 0));
		}
		wait 0.05;
	}
}

//	A player's attached models, as ` <tag>:<model>` words: the gear the dm
//	script drew, which a frag off his back strikes.
attached(p)
{
	out = "";
	for (i = 0; i < p getAttachSize(); i++)
		out = out + " " + p getAttachTagName(i) + ":" + p getAttachModelName(i);
	return out;
}

//	The segments the surfs part traces for `name`, start and end in turn, in
//	the model's frame: the first two of each list are controls, one surface
//	alone in the way.
surf_segs(name)
{
	s = [];
	if (name == "xmodel/vehicle_german_kubelwagen")
	{
		s[s.size] = (0, 0, 120);
		s[s.size] = (0, 0, -40);
		s[s.size] = (120, 0, 30);
		s[s.size] = (-40, 0, 30);
		s[s.size] = (46.90625, -28.984375, 15.484375);
		s[s.size] = (63.328125, -31.890625, -3.96875);
		s[s.size] = (1.640625, 26.296875, 47.875);
		s[s.size] = (2.859375, 29.234375, 38.71875);
		s[s.size] = (46.8125, 29.09375, 17.28125);
		s[s.size] = (56.890625, 30.828125, 10.296875);
		s[s.size] = (1.125, 34.390625, 29.359375);
		s[s.size] = (-3.578125, 5.875, 35.546875);
		s[s.size] = (28.21875, -15.1875, 41.4375);
		s[s.size] = (19.015625, -15.109375, 69.5);
		s[s.size] = (21.796875, -6.359375, 61.609375);
		s[s.size] = (26.328125, -4.015625, 47.359375);
		s[s.size] = (0.265625, 28.625, 44.390625);
		s[s.size] = (-4.25, 11.015625, 43.671875);
		s[s.size] = (3.109375, 26.359375, 18.0625);
		s[s.size] = (-10.046875, 26.1875, 45.015625);
	}
	else if (name == "xmodel/mp_vehicle_panzeriv")
	{
		s[s.size] = (0, 0, 160);
		s[s.size] = (0, 0, -40);
		s[s.size] = (200, 0, 40);
		s[s.size] = (-60, 0, 40);
		s[s.size] = (-129.71875, -59.171875, 41.9375);
		s[s.size] = (-125.9375, -62.15625, 35.75);
		s[s.size] = (-129.8125, 44.6875, 41.59375);
		s[s.size] = (-125.140625, 41.9375, 36.15625);
		s[s.size] = (-8.015625, -44.234375, 0.125);
		s[s.size] = (-16.4375, -52.875, 4.609375);
		s[s.size] = (98.140625, 68.9375, 10.34375);
		s[s.size] = (105.203125, 47.015625, 22.65625);
		s[s.size] = (115.0625, 71.015625, 54.90625);
		s[s.size] = (111.046875, 55.015625, 29.375);
		s[s.size] = (64.171875, 69.453125, 60.90625);
		s[s.size] = (71.671875, 53.8125, 42.171875);
	}
	return s;
}

//	`p` in the model's frame to the world, the model at `at` turned `yaw`
//	0 or 90.
surf_world(at, yaw, p)
{
	if (yaw == 90)
		p = (0 - p[1], p[0], p[2]);
	return at + p;
}

surf_traces(phase, at, yaw, segs)
{
	for (i = 0; i + 1 < segs.size; i += 2)
	{
		tr = bullettrace(surf_world(at, yaw, segs[i]), surf_world(at, yaw, segs[i + 1]), true, undefined);
		logPrint("PROBE strace " + gettime() + " " + phase + " " + i / 2 + " " + entnum(tr["entity"]) + " " + tr["fraction"] + " " + tr["normal"] + " " + tr["position"] + "\n");
	}
}

surfs_part(name)
{
	wait 2;
	segs = surf_segs(name);
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	if (!spawns.size)
		spawns = getentarray("mp_tdm_spawn", "classname");
	at = undefined;
	for (i = 0; i < spawns.size && !isdefined(at); i++)
	{
		o = spawns[i].origin;
		if (bullettrace(o + (0, 0, 16), o + (0, 0, 600), false, undefined)["fraction"] == 1)
			at = o + (0, 0, 300);
	}
	if (!isdefined(at))
	{
		logPrint("PROBE smodel none\n");
		return;
	}
	yaws = [];
	yaws[0] = 0;
	yaws[1] = 90;
	for (k = 0; k < yaws.size; k++)
	{
		model = spawn("script_model", at);
		model setmodel(name);
		model.angles = (0, yaws[k], 0);
		phase = "yaw" + yaws[k];
		logPrint("PROBE smodel " + gettime() + " " + phase + " " + model getEntityNumber() + " " + model.origin + "\n");
		wait 0.1;
		surf_traces(phase, at, yaws[k], segs);
		model delete();
		wait 0.1;
		surf_traces("world" + yaws[k], at, yaws[k], segs);
	}
	logPrint("PROBE sdone " + gettime() + "\n");
}
