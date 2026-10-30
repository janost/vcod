// vcod's killcam probe for mp_crash, loaded over the map's own effects
// script: the killcam on. `killedBy` runs on the victim: the named player
// kills him through the stock damage callback, as `_dev.gsc`'s does, so
// the kill goes through `Callback_PlayerKilled` and its killcam.
// `progressBar` puts the stock primary progress bar on the player and
// fills it in two seconds, as `_gameobjects::useHoldThinkLoop` does.
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
