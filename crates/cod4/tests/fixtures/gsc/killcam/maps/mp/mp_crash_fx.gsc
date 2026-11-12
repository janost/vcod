// vcod's killcam probe for mp_crash, loaded over the map's own effects
// script: the killcam on. `killedBy` runs on the victim: the named player
// kills him through the stock damage callback, as `_dev.gsc`'s does, so
// the kill goes through `Callback_PlayerKilled` and its killcam.
// `progressBar` puts the stock primary progress bar on the player and
// fills it in two seconds, as `_gameobjects::useHoldThinkLoop` does.
// `camo` gives the named weapon on its `gunModel[3]`, as a custom class's
// camo does, and switches to it. `near` stands the player 64 units off
// the named player, where the other's snapshots carry his body. `drop`
// throws the named weapon down as an item.
main()
{
	setDvar( "scr_game_allowkillcam", 1 );
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

progressBar( unused )
{
	bar = self maps\mp\gametypes\_hud_util::createPrimaryProgressBar();
	bar maps\mp\gametypes\_hud_util::updateBar( 0, 0.5 );
}

camo( name )
{
	self giveWeapon( name, 3 );
	self setSpawnWeapon( name );
}

near( name )
{
	for ( i = 0; i < level.players.size; i++ )
	{
		player = level.players[i];
		if ( isPlayer( player ) && player.name == name )
			self setOrigin( player.origin + ( 64, 0, 0 ) );
	}
}

drop( name )
{
	self dropItem( name );
}
