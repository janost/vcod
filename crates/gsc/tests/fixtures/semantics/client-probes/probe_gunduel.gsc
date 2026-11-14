//	Two gunners in each other's arc (cod11-turrets.md 13.10). Stock tdm on
//	mp_carentan, any patch.
//
//	Two `--probe-uo-weapon direct` clients, allies then axis. The allied
//	player mans the gun nearest (1712 1830 8), gun A; the script moves the
//	map's other misc_mg42, gun B, 300 units in front of A facing back, at
//	A's height over the ground. Each player stands 40 units behind his gun
//	facing along it, held there until the script mounts him (`useby`) at
//	level time 20000. From then the script sets both views every frame and
//	drives both triggers through `iPrintLn("PROBE_IN <buttons> 0 0")`:
//
//	- `turn_b`: the axis gunner sweeps his gun from 40 right to 40 left and
//	  back, 4 degrees a frame; the allied gunner holds the trigger on his
//	  chest.
//	- `turn_a`: the same with the roles swapped.
//	- `duel`: each holds the trigger on the other's chest; 0.5 s in, every
//	  hit kills.
//
//	PROBE gun <name> <entity number> <origin> <angles>
//	PROBE player <entity number> <team>
//	PROBE spawnat <entity number> <x> <y> <z> <pitch> <yaw> <roll>
//	PROBE phase <time> <name>
//
//	`spawnat` is the tdm spawn point nearest the player at his first
//	`setOrigin`, which a replay pins. Before the lethal step a hit takes no
//	health; every hit logs its point and direction to the float's last bit
//	(probe_turret's fine1), then a 5 by 5 grid of level `bulletTrace`s
//	(hitCharacters on) along the round, 2 units apart, centred on the point,
//	from 32 before it to 32 past it, which reads the pose the round met. A
//	cell is the distance to where the ray stopped on the victim, or `-`:
//
//	PROBE hit <time> <victim> <attacker> <location> <damage> <point> <dir>
//	PROBE grid <time> <victim> <row> <cell>...
//	PROBE lethal <time>
//
//	Every frame from the mount, each gunner's origin as script reads it
//	(the placement his last end frame left), to the last bit:
//
//	PROBE at <time> <entity number> <origin>

main()
{
	thread run();
	maps\mp\gametypes\tdm::main();
	level.probe_next = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::probe_damage;
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	by = -1;
	if (isdefined(eAttacker))
		by = eAttacker getEntityNumber();
	me = self getEntityNumber();
	logPrint("PROBE hit " + gettime() + " " + me + " " + by + " " + sHitLoc + " " + iDamage + " " + fine(vPoint) + " " + fine(vDir) + "\n");
	self grid(vPoint, vDir);
	if (level.probe_lethal)
		self [[level.probe_next]](eInflictor, eAttacker, 1000, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
}

grid(point, dir)
{
	right = vectornormalize((dir[1], 0 - dir[0], 0));
	up = (right[1] * dir[2], 0 - right[0] * dir[2], right[0] * dir[1] - right[1] * dir[0]);
	for (i = -2; i <= 2; i++)
	{
		line = "PROBE grid " + gettime() + " " + self getEntityNumber() + " " + i;
		for (j = -2; j <= 2; j++)
		{
			c = point + (right[0] * j * 2 + up[0] * i * 2, right[1] * j * 2 + up[1] * i * 2, right[2] * j * 2 + up[2] * i * 2);
			start = c - (dir[0] * 32, dir[1] * 32, dir[2] * 32);
			tr = bullettrace(start, c + (dir[0] * 32, dir[1] * 32, dir[2] * 32), true, undefined);
			if (!isdefined(tr["entity"]) || tr["entity"] != self || tr["fraction"] >= 1)
				line = line + " -";
			else
				line = line + " " + fine1(tr["fraction"] * 64);
		}
		logPrint(line + "\n");
	}
}

fine(v)
{
	return fine1(v[0]) + "," + fine1(v[1]) + "," + fine1(v[2]);
}

//	probe_turret's fine1: whole part by halving steps, then thousandths and
//	millionths of a thousandth.
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

ground_at(p)
{
	tr = bullettrace(p + (0, 0, 64), p + (0, 0, -128), false, undefined);
	return tr["position"];
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

log_spawn()
{
	if (isdefined(self.probe_spawnat))
		return;
	self.probe_spawnat = 1;
	logPrint("PROBE player " + self getEntityNumber() + " " + self.pers["team"] + "\n");
	spawns = getentarray("mp_teamdeathmatch_spawn", "classname");
	if (!spawns.size)
		spawns = getentarray("mp_tdm_spawn", "classname");
	best = undefined;
	for (i = 0; i < spawns.size; i++)
		if (!isdefined(best) || distance(spawns[i].origin, self.origin) < distance(best.origin, self.origin))
			best = spawns[i];
	if (isdefined(best))
	{
		o = best.origin;
		a = best.angles;
		logPrint("PROBE spawnat " + self getEntityNumber() + " " + o[0] + " " + o[1] + " " + o[2] + " " + a[0] + " " + a[1] + " " + a[2] + "\n");
	}
}

//	`up` stays 0: both gunners stand.
send_in(player, buttons)
{
	if (isdefined(player))
		player iprintln("PROBE_IN " + buttons + " 0 0");
}

//	The view from `from`'s gun at `to`'s chest.
aim_at(from, gun, to)
{
	return vectortoangles(to.origin + (0, 0, 40) - (gun.origin + (0, 0, 8)));
}

//	Holds both players on their spots until the mount, then sets both views
//	every frame as the phase asks.
hold(site)
{
	for (;;)
	{
		a = team_player("allies");
		b = team_player("axis");
		if (!level.probe_mounted)
		{
			if (isdefined(a) && distance(a.origin, site.a) > 0.5)
			{
				a log_spawn();
				a setorigin(site.a);
				a setplayerangles((0, site.gun_a.angles[1], 0));
			}
			if (isdefined(b) && distance(b.origin, site.b) > 0.5)
			{
				b log_spawn();
				b setorigin(site.b);
				b setplayerangles((0, site.gun_b.angles[1], 0));
			}
		}
		else if (isdefined(a) && isdefined(b))
		{
			logPrint("PROBE at " + gettime() + " " + a getEntityNumber() + " " + fine(a.origin) + "\n");
			logPrint("PROBE at " + gettime() + " " + b getEntityNumber() + " " + fine(b.origin) + "\n");
			va = (0, site.gun_a.angles[1], 0);
			vb = (0, site.gun_b.angles[1], 0);
			if (level.probe_phase == "turn_b" || level.probe_phase == "duel")
				va = aim_at(a, site.gun_a, b);
			if (level.probe_phase == "turn_a" || level.probe_phase == "duel")
				vb = aim_at(b, site.gun_b, a);
			if (level.probe_phase == "turn_b")
				vb = (0, site.gun_b.angles[1] + level.probe_sweep, 0);
			if (level.probe_phase == "turn_a")
				va = (0, site.gun_a.angles[1] + level.probe_sweep, 0);
			a setplayerangles(va);
			b setplayerangles(vb);
		}
		wait 0.05;
	}
}

//	40 right to 40 left and back, 4 degrees a frame.
sweep()
{
	for (s = -40; s <= 40; s += 4)
	{
		level.probe_sweep = s;
		wait 0.05;
	}
	for (s = 40; s >= -40; s -= 4)
	{
		level.probe_sweep = s;
		wait 0.05;
	}
}

phase(name)
{
	level.probe_phase = name;
	logPrint("PROBE phase " + gettime() + " " + name + "\n");
}

run()
{
	level.probe_phase = "none";
	level.probe_mounted = 0;
	level.probe_lethal = 0;
	level.probe_sweep = 0;
	wait 1;
	guns = getentarray("misc_mg42", "classname");
	ga = undefined;
	for (i = 0; i < guns.size; i++)
		if (!isdefined(ga) || distance(guns[i].origin, (1712, 1830, 8)) < distance(ga.origin, (1712, 1830, 8)))
			ga = guns[i];
	gb = undefined;
	for (i = 0; i < guns.size; i++)
		if (guns[i] != ga)
			gb = guns[i];
	if (!isdefined(ga) || !isdefined(gb))
	{
		logPrint("PROBE guns none\n");
		return;
	}
	fwd = anglestoforward((0, ga.angles[1], 0));
	site = spawnstruct();
	site.gun_a = ga;
	site.gun_b = gb;
	site.a = ground_at(ga.origin - (fwd[0] * 40, fwd[1] * 40, 0)) + (0, 0, 0.125);
	height = ga.origin[2] - site.a[2];
	o = ga.origin + (fwd[0] * 300, fwd[1] * 300, 0);
	site.b = ground_at(o + (fwd[0] * 40, fwd[1] * 40, 0)) + (0, 0, 0.125);
	gb.origin = (o[0], o[1], site.b[2] + height);
	gb.angles = (0, ga.angles[1] + 180, 0);
	logPrint("PROBE gun a " + ga getEntityNumber() + " " + ga.origin + " " + ga.angles + "\n");
	logPrint("PROBE gun b " + gb getEntityNumber() + " " + gb.origin + " " + gb.angles + "\n");
	logPrint("PROBE site " + site.a + " " + site.b + "\n");
	thread hold(site);
	for (;;)
	{
		if (isdefined(team_player("allies")) && isdefined(team_player("axis")))
			break;
		wait 0.05;
	}
	//	The mount and every phase start on a fixed level time, so the
	//	gunners' idle clips read the same clock on any server however late
	//	the clients joined.
	while (gettime() < 20000)
		wait 0.05;
	a = team_player("allies");
	b = team_player("axis");
	ga useby(a);
	gb useby(b);
	level.probe_mounted = 1;
	phase("mounted");
	wait 1;
	phase("turn_b");
	send_in(a, 1);
	sweep();
	send_in(a, 0);
	phase("rest");
	wait 0.5;
	phase("turn_a");
	send_in(b, 1);
	sweep();
	send_in(b, 0);
	phase("rest");
	wait 0.5;
	phase("duel");
	send_in(a, 1);
	send_in(b, 1);
	wait 0.5;
	level.probe_lethal = 1;
	logPrint("PROBE lethal " + gettime() + "\n");
	wait 1.5;
	send_in(team_player("allies"), 0);
	send_in(team_player("axis"), 0);
	logPrint("PROBE done " + gettime() + "\n");
}
