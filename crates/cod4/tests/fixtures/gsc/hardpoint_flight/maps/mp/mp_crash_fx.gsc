// vcod's hardpoint flight probe for mp_crash (docs/research/cod4-gsc.md 3.2).
// Loaded over the map's own effects script under an fs_game: retail reads
// a mod's loose scripts first. It drops the map's ambient effects and flies
// two planes and a helicopter on fixed paths, no random draw, so a capture
// replays on vcod-server step for step.
main()
{
	precacheModel( "vehicle_mig29_desert" );
	level thread hp_probe();
}

hp_log( what, ent )
{
	logPrint( "PROBE " + getTime() + " " + what + " " + ent.origin + " " + ent.angles + "\n" );
}

hp_probe()
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
	nodes = [];
	node = getent( loop[0].target, "targetname" );
	for ( i = 0; i < 4 && isDefined( node ); i++ )
	{
		nodes[i] = node;
		if ( !isDefined( node.target ) )
			break;
		node = getent( node.target, "targetname" );
	}
	base = nodes[0].origin;
	logPrint( "PROBE " + getTime() + " nodes " + nodes.size + " " + base + "\n" );

	// Two planes over the first node: one at a steady speed, one easing
	// in and out.
	p1 = spawnPlane( owner, "script_model", base + ( -3000, 0, 400 ) );
	p1 setModel( "vehicle_mig29_desert" );
	p1.angles = ( 0, 0, 0 );
	p1 moveTo( base + ( 3000, 0, 400 ), 4, 0, 0 );
	p2 = spawnPlane( owner, "script_model", base + ( 0, -3000, 600 ) );
	p2 setModel( "vehicle_mig29_desert" );
	p2.angles = ( 0, 90, 0 );
	p2 moveTo( base + ( 0, 3000, 600 ), 5, 1, 1.5 );

	heli = spawnHelicopter( owner, base, nodes[0].angles, "cobra_mp", "vehicle_cobra_helicopter_fly" );
	heli thread hp_notes();
	heli setspeed( 60, 25 );
	heli setyawspeed( 75, 45, 45 );
	heli setmaxpitchroll( 30, 30 );
	heli setneargoalnotifydist( 256 );
	heli setturningability( 0.9 );
	hp_log( "spawn", heli );

	// Along the loop, through the middle nodes, stopping at the last.
	for ( i = 1; i < nodes.size; i++ )
	{
		heli setvehgoalpos( nodes[i].origin, i == nodes.size - 1 );
		heli waittill( "goal" );
		hp_log( "goal" + i, heli );
	}

	// Hover, then a turn on the spot and the gun on a point below.
	wait 4;
	hp_log( "hover", heli );
	heli settargetyaw( heli.angles[1] + 120 );
	heli setturrettargetvec( base + ( 0, 0, -1500 ) );
	wait 4;
	hp_log( "turned", heli );
	heli cleartargetyaw();
	heli clearturrettarget();

	// Back to the first node, slower, with a softer hover and drag.
	heli sethoverparams( 60, 20, 10 );
	heli setairresistance( 40 );
	heli setspeed( 40, 15, 30 );
	heli setvehgoalpos( base, 1 );
	heli waittill( "goal" );
	hp_log( "home", heli );
	wait 3;
	hp_log( "end", heli );
	heli delete();
	p1 delete();
	p2 delete();
	logPrint( "PROBE " + getTime() + " done\n" );
}

hp_notes()
{
	self endon( "death" );
	for ( ;; )
	{
		self waittill( "near_goal" );
		hp_log( "near_goal", self );
	}
}
