// C4-117 probe for mp_crash: a script_origin loops a sound beside the
// first player and turns a full circle about its yaw over four seconds.
main()
{
	level thread c117_probe();
}

c117_probe()
{
	for ( ;; )
	{
		if ( isDefined( level.players ) && level.players.size > 0 && level.players[0].sessionstate == "playing" )
			break;
		wait 0.05;
	}
	p = level.players[0];
	wait 1;
	e = spawn( "script_origin", p.origin + ( 200, 0, 40 ) );
	e playLoopSound( "mp_cobra_helicopter" );
	wait 1;
	e rotateYaw( 360, 4 );
}
