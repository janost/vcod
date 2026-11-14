//	An automatic weapon's trigger held through a melee swing
//	(docs/research/cod11-combat.md 1.4, "The unlatched arm"). Stock tdm on
//	mp_carentan under tools/run_probe.sh, one `--probe-uo-weapon direct`
//	axis client with `--probe-weapon mp40_mp`. The script holds him on the
//	first tdm spawn point's ground looking along the first heading with
//	level ground and a clear line, and drives his buttons through
//	`iPrintLn("PROBE_IN ...")`; his damage callback takes nothing. He
//	holds the trigger, taps melee with it held (`probe_melee` ms in,
//	default 400, for 100 ms) and keeps it held to 2 s. Every frame his
//	primary clip changes is logged, so each round shows on the frame its
//	cmd ran in:
//
//	PROBE site <spot> <yaw>
//	PROBE in <time> <buttons>
//	PROBE clip <time> <rounds>

main()
{
	thread run();
	maps\mp\gametypes\tdm::main();
	level.callbackPlayerDamage = ::no_damage;
}

no_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
}

in(buttons)
{
	level.probe_player iprintln("PROBE_IN " + buttons + " 0 0");
	logPrint("PROBE in " + gettime() + " " + buttons + "\n");
}

ground_at(p)
{
	tr = bullettrace(p + (0, 0, 64), p + (0, 0, -256), false, undefined);
	return tr["position"];
}

hold()
{
	last = -1;
	for (;;)
	{
		self setorigin(level.spot);
		self setplayerangles(level.aim);
		c = self getweaponslotclipammo("primary");
		if (c != last)
			logPrint("PROBE clip " + gettime() + " " + c + "\n");
		last = c;
		wait 0.05;
	}
}

run()
{
	wait 1;
	sp = getentarray("mp_teamdeathmatch_spawn", "classname");
	spot = ground_at(sp[0].origin);
	yaw = 0;
	for (i = 0; i < 8; i++)
	{
		f = anglestoforward((0, i * 45, 0));
		g = ground_at(spot + (f[0] * 160, f[1] * 160, 0));
		if (absf(g[2] - spot[2]) < 1 && bullettrace(spot + (0, 0, 60), g + (0, 0, 1), false, undefined)["fraction"] == 1)
		{
			yaw = i * 45;
			break;
		}
	}
	logPrint("PROBE site " + spot + " " + yaw + "\n");
	for (;;)
	{
		pl = getentarray("player", "classname");
		if (pl.size > 0 && pl[0].sessionstate == "playing")
			break;
		wait 0.05;
	}
	p = pl[0];
	level.probe_player = p;
	level.spot = spot;
	level.aim = (0, yaw, 0);
	p thread hold();
	in(0);
	melee = 400;
	if (getcvar("probe_melee") != "")
		melee = getcvarint("probe_melee");
	wait 2;
	in(1);
	wait melee * 0.001;
	in(33);
	wait 0.1;
	in(1);
	wait 1.9 - melee * 0.001;
	in(0);
	wait 1;
	logPrint("PROBE done\n");
}

absf(x)
{
	if (x < 0)
		return 0 - x;
	return x;
}
