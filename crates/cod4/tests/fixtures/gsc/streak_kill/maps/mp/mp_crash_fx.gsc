// vcod's kill streak probe for mp_crash, loaded over the map's own effects
// script: no killcam, so the victim is back in a few seconds, and
// `scr_game_forceuav` 1, so the third kill in a row earns the airstrike
// (`_hardpoints::giveHardpointItemForStreak`). `killedBy` runs on the
// victim: the named player kills him through the stock damage callback,
// as `_dev.gsc`'s does, so the kill goes through `Callback_PlayerKilled`.
main()
{
	setDvar( "scr_game_allowkillcam", 0 );
	setDvar( "scr_game_forceuav", 1 );
}

killedBy( name )
{
	attacker = undefined;
	for ( i = 0; i < level.players.size; i++ )
	{
		player = level.players[i];
		if ( isPlayer( player ) && player.name == name )
			attacker = player;
	}
	self thread [[level.callbackPlayerDamage]]( attacker, attacker, 1000, 0, "MOD_RIFLE_BULLET", attacker getCurrentWeapon(), self.origin, ( 1, 0, 0 ), "torso_upper", 0 );
}

// The named player's victim: set down 100 units ahead of him, so
// his own rounds kill us.
standBefore( name )
{
	attacker = playerNamed( name );
	if ( !isDefined( attacker ) )
		return;
	forward = anglesToForward( ( 0, attacker.angles[1], 0 ) );
	self setOrigin( attacker.origin + vector_scale( forward, 100 ) );
}

// Set down where the airstrike in flight aims, once its danger centre is
// in (`doArtillery`, 2 s after the call).
standAtStrike( unused )
{
	self endon( "disconnect" );
	self endon( "death" );
	while ( !isDefined( level.artilleryDangerCenters ) || !level.artilleryDangerCenters.size )
		wait 0.05;
	self setOrigin( level.artilleryDangerCenters[0].origin );
}

// Set down on the spawn point farthest from where the airstrike in
// flight aims, so our own strike spares us.
standClear( unused )
{
	self endon( "disconnect" );
	self endon( "death" );
	while ( !isDefined( level.artilleryDangerCenters ) || !level.artilleryDangerCenters.size )
		wait 0.05;
	aim = level.artilleryDangerCenters[0].origin;
	spawns = getEntArray( "mp_tdm_spawn", "classname" );
	best = undefined;
	for ( i = 0; i < spawns.size; i++ )
	{
		if ( !isDefined( best ) || distance( spawns[i].origin, aim ) > distance( best.origin, aim ) )
			best = spawns[i];
	}
	if ( isDefined( best ) )
		self setOrigin( best.origin );
}

playerNamed( name )
{
	for ( i = 0; i < level.players.size; i++ )
	{
		player = level.players[i];
		if ( isPlayer( player ) && player.name == name )
			return player;
	}
	return undefined;
}

vector_scale( v, k )
{
	return ( v[0] * k, v[1] * k, v[2] * k );
}
