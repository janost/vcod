// vcod's killcam entity probe for mp_crash (docs/research/cod4-combat.md 7),
// loaded over the map's own effects script under an fs_game: retail reads a
// mod's scripts first. Two players, `target` and `shooter`; once both play,
// a script model with the helicopter's model goes 200 units over the
// shooter's `setviewpos` spot and starts a straight 12 s move; 5 s on the target dies through
// the stock damage callback with that model as the inflictor and
// `cobra_20mm_mp` as the weapon, so `Callback_PlayerKilled` hands the
// killcam its number (`killcamentity`). No random draw.
main()
{
	precacheModel( "vehicle_cobra_helicopter_fly" );
	setDvar( "scr_game_allowkillcam", 1 );
	level thread kc_probe();
}

kc_probe()
{
	for ( ;; )
	{
		target = kc_player( "target" );
		shooter = kc_player( "shooter" );
		if ( isDefined( target ) && isDefined( shooter ) && target.sessionstate == "playing" && shooter.sessionstate == "playing" )
			break;
		wait 0.05;
	}
	cam = spawn( "script_model", ( 123, 2110, 500 ) );
	cam setModel( "vehicle_cobra_helicopter_fly" );
	cam moveTo( cam.origin + ( 600, 0, 0 ), 12 );
	logPrint( "PROBE " + getTime() + " cam " + cam getEntityNumber() + "\n" );
	wait 5;
	target thread [[level.callbackPlayerDamage]]( cam, shooter, 1000, 0, "MOD_RIFLE_BULLET", "cobra_20mm_mp", target.origin, ( 1, 0, 0 ), "torso_upper", 0 );
}

kc_player( name )
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
