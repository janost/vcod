//	Slanted point traces near the ground, for a gdb run on the leaf walk's
//	bounds test (cod_lnxded 0x8054c48) that logs each call's point arm
//	(docs/research/cod11-player-clip.md 8.12): 2 bulletTraces a frame for
//	60 s from random spots over the deathmatch spawns to random spots under
//	them. The traces' results are not logged; gdb reads the arm's inputs and
//	its return. Run by tools/run_probe.sh on mp_carentan with COD_LNXDED at a
//	gdb wrapper; no client needed.

main()
{
	maps\mp\gametypes\dm::main();
	thread probe();
}

probe()
{
	wait 1;
	spawns = getentarray("mp_deathmatch_spawn", "classname");
	logPrint("PROBE spawns " + spawns.size + "\n");
	for (frame = 0; frame < 1200; frame++)
	{
		for (i = 0; i < 2; i++)
		{
			a = spawns[randomint(spawns.size)].origin;
			s = a + (randomint(801) - 400, randomint(801) - 400, randomint(161) - 20);
			e = a + (randomint(801) - 400, randomint(801) - 400, randomint(161) - 120);
			t = bulletTrace(s, e, false, undefined);
		}
		wait 0.05;
	}
	logPrint("PROBE done\n");
}
