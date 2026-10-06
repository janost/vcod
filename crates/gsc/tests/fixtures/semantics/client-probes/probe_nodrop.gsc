//	CONTENTS_NODROP on retail. No stock map carries the bit, so this runs on
//	mp_itemtest, a copy of mp_carentan whose material 0
//	(textures/common/clipmonster) has 0x80000000 added to its contents; the
//	recipe is in this directory's README. Brush 895 is then a nodrop box at
//	(493..547, 1859..1887, -144..-93) over a floor at -143.88. Four carbines:
//	one dropped inside the box, one from above it, one beside it, and one
//	suspended (spawnflags 1) inside it. Each logs whether it still exists
//	and where, every frame for 3 s. Needs no client.

main()
{
	thread run();
	maps\mp\gametypes\dm::main();
}

run()
{
	wait 2;
	logPrint("PROBE start " + getTime() + "\n");
	spawn_one("inside", (520, 1873, -110), 0);
	spawn_one("above", (520, 1873, 20), 0);
	spawn_one("beside", (520, 1840, -110), 0);
	spawn_one("hang", (520, 1873, -120), 1);
	wait 4;
	logPrint("PROBE done " + getTime() + "\n");
}

spawn_one(tag, at, flags)
{
	e = spawn("mpweapon_m1carbine", at, flags);
	logPrint("PROBE spawned " + tag + " " + getTime() + " " + e getEntityNumber() + " " + e.origin + "\n");
	thread watch(tag, e);
}

watch(tag, e)
{
	for (i = 0; i < 60; i++)
	{
		wait 0.05;
		if (!isdefined(e))
		{
			logPrint("PROBE freed " + tag + " " + getTime() + "\n");
			return;
		}
		logPrint("PROBE at " + tag + " " + getTime() + " " + e.origin + "\n");
	}
}
