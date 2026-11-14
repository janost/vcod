// Reads every player's head tag once a frame, so the server builds each
// body's skeleton (and runs its controller) every frame.
main()
{
	level thread ct_loop();
}

ct_loop()
{
	for ( ;; )
	{
		wait 0.05;
		players = getEntArray( "player", "classname" );
		for ( i = 0; i < players.size; i++ )
		{
			if ( isAlive( players[i] ) )
				o = players[i] getTagOrigin( "j_head" );
		}
	}
}
