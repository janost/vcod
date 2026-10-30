// C4-81 probe for mp_crash: two jets over the first player (one with its
// four playFxOnTag in its spawn frame as the stock playPlaneFx, one half a
// second later), then that player drops his gun at his feet (a cursor
// hint) and suicides with the hint up.
main()
{
	precacheModel( "vehicle_mig29_desert" );
	level thread c81_probe();
}

c81_log( what )
{
	logPrint( "PROBE " + getTime() + " " + what + "\n" );
}

c81_planefx()
{
	self endon( "death" );
	playfxontag( level.fx_airstrike_afterburner, self, "tag_engine_right" );
	playfxontag( level.fx_airstrike_afterburner, self, "tag_engine_left" );
	playfxontag( level.fx_airstrike_contrail, self, "tag_right_wingtip" );
	playfxontag( level.fx_airstrike_contrail, self, "tag_left_wingtip" );
}

c81_late()
{
	self endon( "death" );
	wait 0.5;
	c81_log( "late fx " + self getEntityNumber() );
	self c81_planefx();
}

c81_probe()
{
	for ( ;; )
	{
		if ( isDefined( level.players ) && level.players.size > 0 && level.players[0].sessionstate == "playing" )
			break;
		wait 0.05;
	}
	p = level.players[0];
	wait 1;

	p1 = spawnPlane( p, "script_model", p.origin + ( -2500, 0, 900 ) );
	p1 setModel( "vehicle_mig29_desert" );
	p1.angles = ( 0, 0, 0 );
	p1 thread c81_planefx();
	p1 moveTo( p.origin + ( 2500, 0, 900 ), 4, 0, 0 );
	c81_log( "plane " + p1 getEntityNumber() + " fx at spawn" );
	wait 0.05;
	p2 = spawnPlane( p, "script_model", p.origin + ( 0, -2500, 1100 ) );
	p2 setModel( "vehicle_mig29_desert" );
	p2.angles = ( 0, 90, 0 );
	p2 thread c81_late();
	p2 moveTo( p.origin + ( 0, 2500, 1100 ), 4, 0, 0 );
	c81_log( "plane " + p2 getEntityNumber() + " fx late" );
	wait 4;
	p1 delete();
	p2 delete();

	wait 1;
	w = p getCurrentWeapon();
	c81_log( "drop " + w );
	p dropItem( w );
	wait 1.5;
	c81_log( "suicide" );
	p suicide();
	wait 3;
	c81_log( "done" );
}
