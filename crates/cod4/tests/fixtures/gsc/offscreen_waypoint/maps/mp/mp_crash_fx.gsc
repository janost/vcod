// vcod's offscreen waypoint probe for mp_crash (docs/research/cod4-hud.md
// 16.3), loaded over the map's own effects script. No stock 1.7 script
// passes setWaypoint an offscreen material, so this one does: once two
// players have spawned, each gets a waypoint of his own 2000 units behind
// him.
main()
{
	precacheShader( "objpoint_default" );
	precacheShader( "waypoint_targetneutral" );
	level thread wp_probe();
}

wp_probe()
{
	for ( ;; )
	{
		if ( isDefined( level.players ) && level.players.size > 1 && level.players[0].sessionstate == "playing" && level.players[1].sessionstate == "playing" )
			break;
		wait 0.05;
	}
	wait 1;
	for ( i = 0; i < level.players.size; i++ )
	{
		player = level.players[i];
		at = player.origin - anglesToForward( player.angles ) * 2000 + ( 0, 0, 64 );
		wp = newClientHudElem( player );
		wp.x = at[0];
		wp.y = at[1];
		wp.z = at[2];
		wp.alpha = 1;
		wp setShader( "objpoint_default", 8, 8 );
		wp setWaypoint( true, "waypoint_targetneutral" );
		logPrint( "PROBE " + getTime() + " waypoint " + i + " " + at + "\n" );
	}
}
