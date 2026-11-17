// vcod's script trace probe for mp_crash (docs/research/cod4-combat.md 3.3),
// loaded over the map's own effects script under an fs_game. Once `target`
// and `shooter` both play, every frame three bulletTraces from the shooter's
// eye run past the target's origin at 64, 56 and 44 up, each printing the
// entity struck (-1 for none), the point in tenths and the normal in
// thousandths. Nothing reads a tag, so only the traces pose the bodies.
// No random draw.
main()
{
	level thread tt_probe();
}

tt_probe()
{
	for ( ;; )
	{
		target = tt_player( "target" );
		shooter = tt_player( "shooter" );
		if ( isDefined( target ) && isDefined( shooter ) && target.sessionstate == "playing" && shooter.sessionstate == "playing" )
			break;
		wait 0.05;
	}
	iPrintLn( "PROBE " + getTime() + " armed" );
	heights = [];
	heights[0] = 64;
	heights[1] = 56;
	heights[2] = 44;
	for ( ;; )
	{
		wait 0.05;
		line = "PROBE " + getTime();
		if ( isDefined( target ) && isDefined( shooter ) && isAlive( target ) && isAlive( shooter ) )
		{
			from = shooter.origin + ( 0, 0, 60 );
			for ( i = 0; i < heights.size; i++ )
			{
				to = target.origin + ( 0, 0, heights[i] );
				t = bulletTrace( from, from + ( to - from ) * 2, true, shooter );
				hit = -1;
				if ( isDefined( t["entity"] ) )
					hit = t["entity"] getEntityNumber();
				n = t["normal"];
				line = line + " " + hit + tt_vec( t["position"] ) + " " + int( n[0] * 1000 ) + " " + int( n[1] * 1000 ) + " " + int( n[2] * 1000 );
			}
		}
		iPrintLn( line );
	}
}

tt_vec( v )
{
	return " " + int( v[0] * 10 ) + " " + int( v[1] * 10 ) + " " + int( v[2] * 10 );
}

tt_player( name )
{
	if ( !isDefined( level.players ) )
		return undefined;
	for ( i = 0; i < level.players.size; i++ )
	{
		player = level.players[i];
		if ( isPlayer( player ) && player.name == name )
			return player;
	}
	return undefined;
}
