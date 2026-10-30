// vcod's round-through-an-entity probe for mp_backlot
// (docs/research/cod4-combat.md 13.1), loaded over the map's own effects
// script. The first destructible sedan stands between the two players:
// the allies player is set down 80 units off its right side, the axis
// player 260 off its left, each facing the car. Every spawned player gets
// an AK-47 alone and no perk, the axis player six rounds in the clip and
// none in reserve. Player damage is logged and not dealt; the car's own
// damage notifies are logged as they come. The lines go to games_mp.log.
main()
{
	level thread pe_hook();
	level thread pe_connect();
	level thread pe_car_log();
}

pe_car()
{
	ents = getEntArray( "script_model", "classname" );
	for ( i = 0; i < ents.size; i++ )
	{
		if ( isSubStr( ents[i].model, "sedan1" ) && isSubStr( ents[i].model, "_destructible" ) )
			return ents[i];
	}
	return undefined;
}

pe_hook()
{
	wait 1;
	level.pe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::pe_damage;
}

pe_connect()
{
	for ( ;; )
	{
		level waittill( "connected", player );
		player thread pe_spawn();
	}
}

pe_spawn()
{
	self endon( "disconnect" );
	for ( ;; )
	{
		self waittill( "spawned_player" );
		wait 0.05;
		self takeAllWeapons();
		self clearPerks();
		self giveWeapon( "ak47_mp" );
		self setSpawnWeapon( "ak47_mp" );
		car = pe_car();
		right = anglesToRight( car.angles );
		if ( self.pers["team"] == "allies" )
		{
			self setWeaponAmmoClip( "ak47_mp", 30 );
			self setWeaponAmmoStock( "ak47_mp", 0 );
			self setOrigin( car.origin + ( right * 80 ) );
		}
		else
		{
			self setWeaponAmmoClip( "ak47_mp", 6 );
			self setWeaponAmmoStock( "ak47_mp", 0 );
			self setOrigin( car.origin - ( right * 260 ) );
		}
		logPrint( "PROBE " + getTime() + " placed " + self getEntityNumber() + " " + pe_milli( self.origin, 1000 ) + " car " + pe_milli( car.origin, 1000 ) + " " + pe_milli( car.angles, 1000 ) + " " + car.model + "\n" );
	}
}

pe_car_log()
{
	wait 0.05;
	car = pe_car();
	for ( ;; )
	{
		car waittill( "damage", amount, attacker, dir, point, mod );
		logPrint( "PROBE " + getTime() + " car " + amount + " " + mod + " " + pe_milli( point, 1000 ) + "\n" );
	}
}

pe_milli( v, scale )
{
	return int( v[0] * scale ) + " " + int( v[1] * scale ) + " " + int( v[2] * scale );
}

pe_damage( eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, psOffsetTime )
{
	logPrint( "PROBE " + getTime() + " hit " + self getEntityNumber() + " " + iDamage + " " + iDFlags + " " + sHitLoc + " " + pe_milli( vPoint, 1000 ) + " " + pe_milli( vDir, 1000000 ) + " " + sMeansOfDeath + "\n" );
}
