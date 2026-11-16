//	`FinishSpawningItem` at the load, for docs/research/cod11-items.md 14.9.
//	No client. Every entity of classname `probe_class` (a +set) is logged
//	each frame from the gametype's first frame for half a second: its
//	number, whether the handle is still live, and its origin. A placed item
//	that starts solid twice is freed by its think; one whose second trace,
//	15 lower, clears is dropped from there.
//
//	PROBE class <classname> <count>
//	PROBE item <time> <index> <num> <isdefined> [<origin>]
//	PROBE done
//	Run by tools/run_probe.sh client-probes/probe_startsolid <map>
//	+set probe_class <classname>.

main()
{
	thread run();
	maps\mp\gametypes\dm::main();
}

run()
{
	class = getcvar("probe_class");
	items = getentarray(class, "classname");
	logPrint("PROBE class " + class + " " + items.size + "\n");
	nums = [];
	for (i = 0; i < items.size; i++)
		nums[i] = items[i] getEntityNumber();
	for (t = 0; t < 10; t++)
	{
		for (i = 0; i < items.size; i++)
		{
			if (isdefined(items[i]))
				logPrint("PROBE item " + getTime() + " " + i + " " + nums[i] + " 1 " + items[i].origin + "\n");
			else
				logPrint("PROBE item " + getTime() + " " + i + " " + nums[i] + " 0\n");
		}
		wait 0.05;
	}
	logPrint("PROBE done\n");
}
