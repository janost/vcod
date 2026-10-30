//	playFxOnTag on a script model (docs/research/cod11-events-and-fx.md 2,
//	"playFxOnTag"): the event rides the model's own entity, not a player's
//	playerState. The first allied player is put on mp_carentan's street at
//	(900 1930 -38) facing north; an mg42_bipod script model stands 96 units
//	ahead, muzzle west. Each step is logged as `PROBE <step> <time> [<entity>]`:
//
//	model       the script model spawned
//	tag_model   mg42hv.efx on its tag_flash
//	tag_case    the same, the tag spelt TAG_FLASH
//	tag_player  mg42hv.efx on the player's tag_helmet
//	done        end of the measured part
//
//	After `done` the model turns and flashes every 0.1 s, and the helmet
//	flashes every 2 s, for a human to watch (pending a hand check).
//
//	The client half is `vcod --net-probe --probe-team allies
//	--save-scripted fx`, whose `!fx` lines fx_ab.rs compares up to `done`.

main()
{
	precacheModel("xmodel/mg42_bipod");
	level._effect["flash"] = loadfx("fx/muzzleflashes/mg42hv.efx");
	thread watch();
	maps\mp\gametypes\dm::main();
}

watch()
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (isdefined(players[i].sessionstate) && players[i].sessionstate == "playing")
			{
				players[i] effects();
				return;
			}
		}
		wait 0.05;
	}
}

effects()
{
	org = (900, 1930, -38);
	self setorigin(org);
	self setplayerangles((0, 90, 0));
	wait 1;

	gun = spawn("script_model", org + (0, 96, 16));
	gun setmodel("xmodel/mg42_bipod");
	gun.angles = (0, 180, 0);
	step("model", gun);
	wait 0.5;
	step("tag_model", undefined);
	playfxontag(level._effect["flash"], gun, "tag_flash");
	wait 0.5;
	step("tag_case", undefined);
	playfxontag(level._effect["flash"], gun, "TAG_FLASH");
	wait 0.5;
	step("tag_player", undefined);
	playfxontag(level._effect["flash"], self, "tag_helmet");
	wait 0.5;
	step("done", undefined);

	self thread helmet();
	for (;;)
	{
		gun rotateyaw(36, 0.1);
		playfxontag(level._effect["flash"], gun, "tag_flash");
		wait 0.1;
	}
}

helmet()
{
	for (;;)
	{
		wait 2;
		if (isdefined(self) && self.sessionstate == "playing")
			playfxontag(level._effect["flash"], self, "tag_helmet");
	}
}

step(name, ent)
{
	if (isdefined(ent))
		logPrint("PROBE " + name + " " + getTime() + " " + ent getEntityNumber() + "\n");
	else
		logPrint("PROBE " + name + " " + getTime() + "\n");
}
