// vcod's tag read probe for mp_crash (docs/research/cod4-combat.md 6.1),
// loaded over the map's own effects script under an fs_game. Once `target`
// and `shooter` both play, every frame prints each living player's j_head,
// and the frame after a `weapon_fired` the shooter's notify thread read
// tag_weapon_right (origin and angles) at, each in tenths. A thread reads a
// player's tag after the cmds that came in since the last end frame moved it.
// No random draw.
main()
{
	level thread tr_probe();
}

tr_probe()
{
	for ( ;; )
	{
		target = tr_player( "target" );
		shooter = tr_player( "shooter" );
		if ( isDefined( target ) && isDefined( shooter ) && target.sessionstate == "playing" && shooter.sessionstate == "playing" )
			break;
		wait 0.05;
	}
	players = [];
	players[0] = target;
	players[1] = shooter;
	for ( i = 0; i < players.size; i++ )
	{
		players[i].tr_fired = "";
		players[i] thread tr_fired();
	}
	iPrintLn( "PROBE " + getTime() + " armed" );
	for ( ;; )
	{
		wait 0.05;
		line = "PROBE " + getTime();
		for ( i = 0; i < players.size; i++ )
		{
			p = players[i];
			if ( !isDefined( p ) )
				continue;
			if ( isAlive( p ) )
				line = line + " " + p getEntityNumber() + tr_vec( p getTagOrigin( "j_head" ) );
			if ( p.tr_fired != "" )
			{
				line = line + " " + p getEntityNumber() + " fired" + p.tr_fired;
				p.tr_fired = "";
			}
		}
		iPrintLn( line );
	}
}

tr_fired()
{
	self endon( "disconnect" );
	for ( ;; )
	{
		self waittill( "weapon_fired" );
		self.tr_fired = self.tr_fired + tr_vec( self getTagOrigin( "tag_weapon_right" ) ) + tr_vec( self getTagAngles( "tag_weapon_right" ) );
	}
}

tr_vec( v )
{
	return " " + int( v[0] * 10 ) + " " + int( v[1] * 10 ) + " " + int( v[2] * 10 );
}

tr_player( name )
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
