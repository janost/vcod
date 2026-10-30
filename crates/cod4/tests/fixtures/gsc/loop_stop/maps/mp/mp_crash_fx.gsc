// C4-85 probe for mp_crash: four script_origins loop a sound beside the
// first player; then one is deleted, one hidden (it leaves the snapshot),
// one stops its loop, and the fourth loops on.
main()
{
	level thread c85_probe();
}

c85_loop( origin, alias )
{
	e = spawn( "script_origin", origin );
	e playLoopSound( alias );
	return e;
}

c85_probe()
{
	for ( ;; )
	{
		if ( isDefined( level.players ) && level.players.size > 0 && level.players[0].sessionstate == "playing" )
			break;
		wait 0.05;
	}
	p = level.players[0];
	wait 1;
	freed = c85_loop( p.origin + ( 200, 0, 40 ), "mp_cobra_helicopter" );
	hidden = c85_loop( p.origin + ( 0, 200, 40 ), "mp_hind_helicopter" );
	stopped = c85_loop( p.origin + ( -200, 0, 40 ), "veh_mig29_dist_loop" );
	kept = c85_loop( p.origin + ( 0, -200, 40 ), "cobra_helicopter_dying_loop" );
	wait 3;
	freed delete();
	wait 2;
	hidden hide();
	wait 2;
	stopped stopLoopSound();
}
