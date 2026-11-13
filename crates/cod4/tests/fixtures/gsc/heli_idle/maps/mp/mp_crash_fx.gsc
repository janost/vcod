// vcod's idle helicopter probe for mp_crash (docs/research/cod4-gsc.md 3.2).
// Loaded over the map's own effects script under an fs_game: retail reads
// a mod's loose scripts first. A cobra spawns over the first loop node
// pitched, rolled and facing past 180, with no goal for its first 4 s, as
// a stock helicopter waits 2 s before its first (`heli_fly`): the think
// runs 0x808ba22 then. Its gun tracks a point ahead and below, then one
// behind it, so the aim runs on a body that holds still. Then it flies a
// pass-through goal and deletes. No hover radius and no stop, so no random
// draw reaches it.
main()
{
	level thread hi_probe();
}

hi_probe()
{
	// The owner: the first player to spawn.
	for ( ;; )
	{
		if ( isDefined( level.players ) && level.players.size > 0 && level.players[0].sessionstate == "playing" )
			break;
		wait 0.05;
	}
	owner = level.players[0];
	wait 1;

	loop = getentarray( "heli_loop_start", "targetname" );
	base = getent( loop[0].target, "targetname" ).origin;
	heli = spawnHelicopter( owner, base, ( 8, 250, -6 ), "cobra_mp", "vehicle_cobra_helicopter_fly" );
	heli setspeed( 40, 20 );
	heli sethoverparams( 0, 10, 5 );
	heli setturrettargetvec( base + ( 900, 500, -700 ) );
	logPrint( "PROBE " + getTime() + " spawn " + heli.origin + " " + heli.angles + "\n" );

	wait 2;
	heli setturrettargetvec( base + ( -600, -900, -300 ) );
	wait 2;
	logPrint( "PROBE " + getTime() + " goal " + heli.origin + " " + heli.angles + "\n" );
	heli setvehgoalpos( base + ( 2000, 600, 200 ), 0 );
	wait 4;
	logPrint( "PROBE " + getTime() + " end " + heli.origin + " " + heli.angles + "\n" );
	heli delete();
	logPrint( "PROBE " + getTime() + " done\n" );
}
