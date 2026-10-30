// vcod's round-on-a-script-model probe for mp_backlot
// (docs/research/cod4-combat.md 13.1), loaded over the map's own effects
// script. The players stand either side of the first destructible sedan
// as under ../pen_ent (the allies player 80 units off its right side, the
// axis player 260 off its left, each turned to face the car half a second
// later), each with an AK-47 alone, no perk and its clip topped up every
// half second. Once the axis player is placed,
// a script model of the sedan's own model stands 150 units off the car's
// left, between the two, in five phases of 2.5 s: `plain`, never opened
// to damage, at the car's height; `raised`, the same 30 units up;
// `attached`, a `tag_origin` model opened to damage with the sedan
// attached at `tag_origin`; `ignored`, the same attached with
// ignoreCollision; then `none`. Player damage is logged and not dealt;
// the car's and the stand-in's damage notifies are logged as they come.
// The lines go to games_mp.log.
main()
{
	precacheModel( "tag_origin" );
	level thread ms_hook();
	level thread ms_connect();
	level thread ms_car_log();
}

ms_car()
{
	ents = getEntArray( "script_model", "classname" );
	for ( i = 0; i < ents.size; i++ )
	{
		if ( isSubStr( ents[i].model, "sedan1" ) && isSubStr( ents[i].model, "_destructible" ) )
			return ents[i];
	}
	return undefined;
}

ms_hook()
{
	wait 1;
	level.ms_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::ms_damage;
}

ms_connect()
{
	for ( ;; )
	{
		level waittill( "connected", player );
		player thread ms_spawn();
	}
}

ms_spawn()
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
		self setWeaponAmmoClip( "ak47_mp", 30 );
		self setWeaponAmmoStock( "ak47_mp", 0 );
		car = ms_car();
		right = anglesToRight( car.angles );
		if ( self.pers["team"] == "allies" )
		{
			self setOrigin( car.origin + ( right * 80 ) );
		}
		else
		{
			self setOrigin( car.origin - ( right * 260 ) );
			level thread ms_phases( car, right );
		}
		self thread ms_refill();
		logPrint( "PROBE " + getTime() + " placed " + self getEntityNumber() + " " + ms_milli( self.origin, 1000 ) + "\n" );
		// Each faces the car on the server's own angles, whatever the
		// view its cmds carried at the spawn.
		wait 0.5;
		if ( self.pers["team"] == "allies" )
			self setPlayerAngles( ( 0, car.angles[1] + 90, 0 ) );
		else
			self setPlayerAngles( ( 0, car.angles[1] - 90, 0 ) );
	}
}

ms_refill()
{
	self endon( "disconnect" );
	self endon( "death" );
	for ( ;; )
	{
		wait 0.5;
		self setWeaponAmmoClip( "ak47_mp", 30 );
	}
}

ms_phases( car, right )
{
	at = car.origin - ( right * 150 );
	ent = ms_phase( "plain", car, at, undefined );
	wait 2.5;
	ent delete();
	ent = ms_phase( "raised", car, at + ( 0, 0, 30 ), undefined );
	wait 2.5;
	ent delete();
	ent = ms_phase( "attached", car, at, false );
	wait 2.5;
	ent delete();
	ent = ms_phase( "ignored", car, at, true );
	wait 2.5;
	ent delete();
	logPrint( "PROBE " + getTime() + " phase none\n" );
}

ms_phase( name, car, at, ignore )
{
	ent = spawn( "script_model", at );
	ent.angles = car.angles;
	if ( isDefined( ignore ) )
	{
		ent setModel( "tag_origin" );
		ent attach( car.model, "tag_origin", ignore );
		ent.health = 100000;
		ent setCanDamage( true );
		ent thread ms_ent_log();
	}
	else
	{
		ent setModel( car.model );
	}
	logPrint( "PROBE " + getTime() + " phase " + name + " " + ent getEntityNumber() + " " + ms_milli( ent.origin, 1000 ) + "\n" );
	return ent;
}

ms_ent_log()
{
	self endon( "death" );
	for ( ;; )
	{
		self waittill( "damage", amount, attacker, dir, point, mod, model, tag, part );
		logPrint( "PROBE " + getTime() + " ent " + amount + " " + mod + " " + ms_milli( point, 1000 ) + " " + model + " " + tag + " " + part + "\n" );
	}
}

ms_car_log()
{
	wait 0.05;
	car = ms_car();
	for ( ;; )
	{
		car waittill( "damage", amount, attacker, dir, point, mod );
		logPrint( "PROBE " + getTime() + " car " + amount + " " + mod + " " + ms_milli( point, 1000 ) + "\n" );
	}
}

ms_milli( v, scale )
{
	return int( v[0] * scale ) + " " + int( v[1] * scale ) + " " + int( v[2] * scale );
}

ms_damage( eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, psOffsetTime )
{
	logPrint( "PROBE " + getTime() + " hit " + self getEntityNumber() + " " + iDamage + " " + iDFlags + " " + sHitLoc + " " + ms_milli( vPoint, 1000 ) + " " + ms_milli( vDir, 1000000 ) + " " + sMeansOfDeath + "\n" );
}
