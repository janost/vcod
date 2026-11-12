// vcod's helicopter collision probe for mp_crash (docs/research/cod4-gsc.md
// 3.2). Loaded over the map's own effects script under an fs_game: retail
// reads a mod's loose scripts first. A cobra spawns over the first loop
// node and holds there with no wander (hover radius 0, so no random draw),
// turns its gun on a point off its left side, and prints its posed
// `tag_barrel` and `tag_flash` and a bullet trace down through the middle
// of the two. Then it flies straight down into the ground: the think's
// move trace notifies `script_vehicle_collision`, and the first three and
// the count are printed. Every line is an `iPrintLn` starting `PROBE`, its
// time counted from the spawn, so a capture carries it.
main()
{
	level thread hc_probe();
}

hc_probe()
{
	// The owner: the first player to spawn.
	for ( ;; )
	{
		if ( isDefined( level.players ) && level.players.size > 0 && level.players[0].sessionstate == "playing" )
			break;
		wait 0.05;
	}
	owner = level.players[0];
	wait 1;

	loop = getentarray( "heli_loop_start", "targetname" );
	base = getent( loop[0].target, "targetname" ).origin;
	heli = spawnHelicopter( owner, base, ( 0, 0, 0 ), "cobra_mp", "vehicle_cobra_helicopter_fly" );
	level.hc_t0 = getTime();
	heli.hc_count = 0;
	heli thread hc_collisions();
	heli setspeed( 30, 15 );
	heli sethoverparams( 0, 10, 5 );
	heli setvehgoalpos( base, 1 );
	iprintln( "PROBE " + hc_time() + " spawn " + base );

	// The gun on a point off the left side and below.
	wait 1;
	heli setturrettargetvec( base + ( 0, 800, -400 ) );
	wait 3;
	barrel = heli gettagorigin( "tag_barrel" );
	flash = heli gettagorigin( "tag_flash" );
	iprintln( "PROBE " + hc_time() + " tags " + barrel + " " + flash );
	mid = ( barrel + flash ) * 0.5;
	t = bulletTrace( mid + ( 0, 0, 64 ), mid - ( 0, 0, 64 ), false, undefined );
	hit = 0;
	if ( isDefined( t["entity"] ) && t["entity"] == heli )
		hit = 1;
	iprintln( "PROBE " + hc_time() + " trace " + hit + " " + t["position"] + " " + t["normal"] );
	heli clearturrettarget();

	// Straight down past the ground.
	heli setvehgoalpos( base - ( 0, 0, 1500 ), 1 );
	wait 12;
	iprintln( "PROBE " + hc_time() + " count " + heli.hc_count + " " + heli.origin );
	heli delete();
	iprintln( "PROBE " + hc_time() + " done" );
}

hc_time()
{
	return getTime() - level.hc_t0;
}

hc_collisions()
{
	self endon( "death" );
	for ( ;; )
	{
		self waittill( "script_vehicle_collision", normal );
		self.hc_count++;
		if ( self.hc_count <= 3 )
			iprintln( "PROBE " + hc_time() + " coll " + normal + " " + self.origin );
	}
}
