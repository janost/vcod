// vcod's damage kick probe for mp_crash (docs/research/cod4-combat.md 3.2):
// the shot aim probe (../shot_aim) with the shooter hurt while it fires.
// Every spawned player gets an AK-47 alone and no perk, its clip topped up
// every half second. The damage callback is wrapped: each round that
// reaches a player logs its point (in thousandths) and its direction (in
// millionths) and does no damage. Ten seconds past its spawn, every 1.3 s
// the player named `shooter` takes `finishPlayerDamage` without knockback
// (iDFlags 4), in turn 40 along its view's forward, 25 along its right
// and 40 with no direction, and is healed at once, so the next rounds
// leave along the damage kick's angles. The lines go to games_mp.log.
main()
{
	level thread sa_hook();
	level thread sa_connect();
}

sa_hook()
{
	wait 1;
	level.sa_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::sa_damage;
}

sa_connect()
{
	for ( ;; )
	{
		level waittill( "connected", player );
		player thread sa_spawn();
	}
}

sa_spawn()
{
	self endon( "disconnect" );
	for ( ;; )
	{
		self waittill( "spawned_player" );
		wait 0.05;
		self takeAllWeapons();
		self clearPerks();
		self giveWeapon( "ak47_mp" );
		self giveMaxAmmo( "ak47_mp" );
		self setSpawnWeapon( "ak47_mp" );
		logPrint( "PROBE " + getTime() + " armed " + self getEntityNumber() + "\n" );
		self thread sa_refill();
		if ( self.name == "shooter" )
			self thread sa_hurt();
	}
}

sa_hurt()
{
	self endon( "disconnect" );
	self endon( "death" );
	wait 10;
	for ( i = 0; ; i++ )
	{
		wait 1.3;
		angles = self getPlayerAngles();
		if ( i % 3 == 0 )
			self finishPlayerDamage( self, self, 40, 4, "MOD_PROJECTILE", "none", self.origin, anglesToForward( angles ), "none", 0 );
		else if ( i % 3 == 1 )
			self finishPlayerDamage( self, self, 25, 4, "MOD_PROJECTILE", "none", self.origin, anglesToRight( angles ), "none", 0 );
		else
			self finishPlayerDamage( self, self, 40, 4, "MOD_PROJECTILE", "none", self.origin, undefined, "none", 0 );
		self.health = self.maxhealth;
		logPrint( "PROBE " + getTime() + " hurt " + self getEntityNumber() + " " + i + "\n" );
	}
}

sa_refill()
{
	self endon( "disconnect" );
	self endon( "death" );
	for ( ;; )
	{
		wait 0.5;
		self setWeaponAmmoClip( "ak47_mp", 30 );
	}
}

sa_milli( v, scale )
{
	return int( v[0] * scale ) + " " + int( v[1] * scale ) + " " + int( v[2] * scale );
}

sa_damage( eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, psOffsetTime )
{
	logPrint( "PROBE " + getTime() + " hit " + self getEntityNumber() + " " + iDamage + " " + iDFlags + " " + sHitLoc + " " + sa_milli( vPoint, 1000 ) + " " + sa_milli( vDir, 1000000 ) + "\n" );
}
