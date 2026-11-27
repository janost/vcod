//	`useBy` on a `trigger_damage` (docs/research/cod11-combat.md 14.8.2):
//	`Use_trigger_damage` activates with `accumulate + 1` as the damage and
//	mod -1. No client. mp_carentan with three submodels' blocks replaced by
//	crates/server/tests/fixtures/triggers/mp_carentan-dmguse.ent, all
//	`accumulate` 100: `use_a` `threshold` 101, `use_b` 102, `use_c` none.
//	A blast under the threshold (or the accumulate) comes off `health` and
//	is refused, so `health` stays down; an activation sets it back to 32000.
//	So after a 100 blast `useBy` puts `use_a` back (101 is not under 101)
//	and leaves `use_b` down (101 is under 102); `use_c` shows the count
//	comes off nothing.
//
//	PROBE phase <time> <name>
//	PROBE health <time> <targetname> <health>
//	PROBE damage <time> <targetname> <dmg> <attacker>
//	PROBE trigger <time> <targetname> <activator>

main()
{
	thread run();
	maps\mp\gametypes\tdm::main();
}

who(e)
{
	if (!isdefined(e))
		return "undefined";
	if (isplayer(e))
		return "player";
	if (isdefined(e.targetname))
		return e.targetname;
	return e.classname;
}

watch()
{
	self endon("death");
	for (;;)
	{
		self waittill("damage", dmg, attacker);
		logPrint("PROBE damage " + gettime() + " " + self.targetname + " " + dmg + " " + who(attacker) + "\n");
	}
}

watch_trigger()
{
	self endon("death");
	for (;;)
	{
		self waittill("trigger", other);
		logPrint("PROBE trigger " + gettime() + " " + self.targetname + " " + who(other) + "\n");
	}
}

health(t)
{
	logPrint("PROBE health " + gettime() + " " + t.targetname + " " + t.health + "\n");
}

blast(t, dmg)
{
	logPrint("PROBE phase " + gettime() + " blast_" + t.targetname + "_" + dmg + "\n");
	radiusDamage(t.origin, 16, dmg, dmg);
	health(t);
	wait 1;
}

use(t, user)
{
	logPrint("PROBE phase " + gettime() + " use_" + t.targetname + "\n");
	t useby(user);
	health(t);
	wait 1;
}

run()
{
	wait 1;
	user = spawn("script_origin", (0, 0, 0));
	user.targetname = "user";
	names = [];
	names[0] = "use_a";
	names[1] = "use_b";
	names[2] = "use_c";
	t = [];
	for (i = 0; i < names.size; i++)
	{
		t[i] = getent(names[i], "targetname");
		t[i] thread watch();
		t[i] thread watch_trigger();
		health(t[i]);
	}
	wait 0.5;
	// At full health: the accumulate gate refuses every use.
	use(t[0], user);
	blast(t[0], 100);
	use(t[0], user);
	blast(t[1], 100);
	use(t[1], user);
	blast(t[2], 50);
	use(t[2], user);
	use(t[2], user);
	blast(t[2], 50);
	logPrint("PROBE done\n");
}
