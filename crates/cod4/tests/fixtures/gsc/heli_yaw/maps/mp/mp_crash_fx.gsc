// vcod's helicopter yaw probe for mp_crash (docs/research/cod4-gsc.md 3.2).
// Loaded over the map's own effects script under an fs_game: retail reads
// a mod's loose scripts first. A cobra with no hover radius (so no random
// draw) flies the yaw's arms: a descending stop turning to a goal yaw, a
// new goal yaw while hovering, a climbing pass-through under a target yaw
// with overshoot, a sharp turn at speed, a stop entered at speed with a
// goal yaw, then `setLookAtEnt` on a script_origin while hovering and
// while flying (the goal yaw still set), the point jumping once, and the
// goal yaw again once the look-at clears.
main()
{
	level thread hy_probe();
}

hy_log( what, ent )
{
	logPrint( "PROBE " + getTime() + " " + what + " " + ent.origin + " " + ent.angles + "\n" );
}

hy_probe()
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
	mark = spawn( "script_origin", base + ( 600, 1800, -300 ) );
	heli = spawnHelicopter( owner, base, ( 0, 30, 0 ), "cobra_mp", "vehicle_cobra_helicopter_fly" );
	heli thread hy_notes();
	heli setspeed( 50, 20 );
	heli setyawspeed( 60, 30, 20, 0.3 );
	heli setmaxpitchroll( 25, 25 );
	heli sethoverparams( 0, 10, 5 );
	heli setneargoalnotifydist( 300 );
	hy_log( "spawn", heli );
	wait 1;

	// A descending stop with no hover radius, turning to a goal yaw.
	heli setgoalyaw( 150 );
	heli setvehgoalpos( base + ( 1500, -600, -250 ), 1 );
	heli waittill( "goal" );
	hy_log( "stop1", heli );
	// Hovering, a goal yaw the other way.
	heli setgoalyaw( -100 );
	wait 4;
	hy_log( "hover1", heli );

	// A climbing pass-through under a target yaw.
	heli cleargoalyaw();
	heli settargetyaw( -60 );
	heli setvehgoalpos( base + ( 2600, 400, 300 ), 0 );
	heli waittill( "goal" );
	hy_log( "pass1", heli );
	heli cleartargetyaw();
	// A sharp turn at speed.
	heli setvehgoalpos( base + ( 2400, 2000, 200 ), 0 );
	heli waittill( "goal" );
	hy_log( "pass2", heli );
	// A stop entered at speed, with a goal yaw.
	heli setgoalyaw( 45 );
	heli setvehgoalpos( base + ( 1000, 2400, 0 ), 1 );
	heli waittill( "goal" );
	hy_log( "stop2", heli );
	wait 1;

	// Looking at the point while hovering, then the point jumps.
	heli setlookatent( mark );
	wait 3;
	mark.origin = base + ( 2200, 3400, -300 );
	wait 3;
	hy_log( "look1", heli );
	// Looking at it while flying, the goal yaw still set.
	heli setvehgoalpos( base + ( -200, 1200, 100 ), 1 );
	heli waittill( "goal" );
	hy_log( "look2", heli );
	wait 1;
	// The look-at cleared: the goal yaw again.
	heli clearlookatent();
	wait 4;
	hy_log( "end", heli );
	heli delete();
	mark delete();
	logPrint( "PROBE " + getTime() + " done\n" );
}

hy_notes()
{
	self endon( "death" );
	self thread hy_yaw_notes();
	for ( ;; )
	{
		self waittill( "near_goal" );
		hy_log( "near_goal", self );
	}
}

hy_yaw_notes()
{
	self endon( "death" );
	for ( ;; )
	{
		self waittill( "goal_yaw" );
		hy_log( "goal_yaw", self );
	}
}
