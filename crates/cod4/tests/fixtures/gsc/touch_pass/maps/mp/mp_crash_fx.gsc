// vcod's touch pass probe for mp_crash (docs/research/cod4-pmove.md 15.2),
// loaded over the map's own effects script under an fs_game: retail reads a
// mod's scripts first. Once `target` and `walker` both play, each counts the
// "touch" notifies it is sent and with whom; every frame a player whose count
// moved has it printed to everyone, with the frame's time. No random draw.
main()
{
	level thread tp_probe();
}

tp_probe()
{
	for ( ;; )
	{
		target = tp_player( "target" );
		walker = tp_player( "walker" );
		if ( isDefined( target ) && isDefined( walker ) && target.sessionstate == "playing" && walker.sessionstate == "playing" )
			break;
		wait 0.05;
	}
	players = [];
	players[0] = target;
	players[1] = walker;
	for ( i = 0; i < players.size; i++ )
	{
		players[i].tp_count = 0;
		players[i].tp_other = -1;
		players[i] thread tp_count();
	}
	iPrintLn( "PROBE " + getTime() + " armed" );
	for ( ;; )
	{
		wait 0.05;
		for ( i = 0; i < players.size; i++ )
		{
			p = players[i];
			if ( p.tp_count == 0 )
				continue;
			iPrintLn( "PROBE " + getTime() + " " + p getEntityNumber() + " touch " + p.tp_other + " x" + p.tp_count );
			p.tp_count = 0;
		}
	}
}

tp_count()
{
	for ( ;; )
	{
		self waittill( "touch", other );
		self.tp_count++;
		self.tp_other = other getEntityNumber();
	}
}

tp_player( name )
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
