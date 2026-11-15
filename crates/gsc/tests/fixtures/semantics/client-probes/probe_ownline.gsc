//	Whether a gunner's round ever meets his own body (cod11-turrets.md 6.3:
//	the round's pass entity is the gun, so his body is not passed). Stock
//	tdm on mp_carentan, any patch, one `--probe-uo-weapon direct` allied
//	client. The script stands him 40 units behind the gun nearest
//	(1712 1830 8) facing along it, mounts him (`useby`) at level time 20000,
//	holds his trigger through `iPrintLn("PROBE_IN 1 0 0")` and sets his view
//	every frame: pitch -40 to 60 in steps of 10, each row a yaw sweep from
//	60 right to 60 left of the gun, 4 degrees a frame, past both arcs.
//	Every damage callback is logged and takes no health; every frame from
//	the mount, his origin as script reads it (the placement his last end
//	frame left), to the last bit:
//
//	PROBE gun <entity number> <origin> <angles>
//	PROBE player <entity number> <time of his first placement>
//	PROBE row <time> <pitch>
//	PROBE hit <time> <victim> <attacker> <location> <damage>
//	PROBE at <time> <entity number> <origin>
//	PROBE done <time>

main()
{
	thread run();
	maps\mp\gametypes\tdm::main();
	level.callbackPlayerDamage = ::probe_damage;
}

probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	by = -1;
	if (isdefined(eAttacker))
		by = eAttacker getEntityNumber();
	logPrint("PROBE hit " + gettime() + " " + self getEntityNumber() + " " + by + " " + sHitLoc + " " + iDamage + "\n");
}

ground_at(p)
{
	tr = bullettrace(p + (0, 0, 64), p + (0, 0, -128), false, undefined);
	return tr["position"];
}

gunner()
{
	players = getentarray("player", "classname");
	for (i = 0; i < players.size; i++)
	{
		p = players[i];
		if (isdefined(p.sessionstate) && p.sessionstate == "playing")
			return p;
	}
	return undefined;
}

//	Holds him on his spot until the mount, then sets his view every frame.
hold(gun, spot)
{
	for (;;)
	{
		a = gunner();
		if (!level.probe_mounted)
		{
			if (isdefined(a) && distance(a.origin, spot) > 0.5)
			{
				if (!isdefined(a.probe_logged))
				{
					a.probe_logged = 1;
					logPrint("PROBE player " + a getEntityNumber() + " " + gettime() + "\n");
				}
				a setorigin(spot);
				a setplayerangles((0, gun.angles[1], 0));
			}
		}
		else if (isdefined(a))
		{
			logPrint("PROBE at " + gettime() + " " + a getEntityNumber() + " " + fine(a.origin) + "\n");
			a setplayerangles((level.probe_pitch, gun.angles[1] + level.probe_yaw, 0));
		}
		wait 0.05;
	}
}

run()
{
	level.probe_mounted = 0;
	level.probe_pitch = 0;
	level.probe_yaw = 0;
	wait 1;
	guns = getentarray("misc_mg42", "classname");
	gun = undefined;
	for (i = 0; i < guns.size; i++)
		if (!isdefined(gun) || distance(guns[i].origin, (1712, 1830, 8)) < distance(gun.origin, (1712, 1830, 8)))
			gun = guns[i];
	if (!isdefined(gun))
	{
		logPrint("PROBE guns none\n");
		return;
	}
	fwd = anglestoforward((0, gun.angles[1], 0));
	spot = ground_at(gun.origin - (fwd[0] * 40, fwd[1] * 40, 0)) + (0, 0, 0.125);
	logPrint("PROBE gun " + gun getEntityNumber() + " " + gun.origin + " " + gun.angles + "\n");
	thread hold(gun, spot);
	while (!isdefined(gunner()))
		wait 0.05;
	//	The mount on a fixed level time, so the idle reads the same clock on
	//	any server however late the client joined.
	while (gettime() < 20000)
		wait 0.05;
	a = gunner();
	gun useby(a);
	level.probe_mounted = 1;
	wait 1;
	//	`probe_nofire 1` sweeps with the trigger up.
	if (getcvar("probe_nofire") != "1")
		a iprintln("PROBE_IN 1 0 0");
	for (p = -40; p <= 60; p += 10)
	{
		level.probe_pitch = p;
		logPrint("PROBE row " + gettime() + " " + p + "\n");
		for (y = 60; y >= -60; y -= 4)
		{
			level.probe_yaw = y;
			wait 0.05;
		}
	}
	a iprintln("PROBE_IN 0 0 0");
	logPrint("PROBE done " + gettime() + "\n");
}

//	probe_turret's fine1: whole part by halving steps, then thousandths and
//	millionths of a thousandth.
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
