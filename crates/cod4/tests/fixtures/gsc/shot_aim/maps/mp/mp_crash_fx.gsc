// vcod's shot aim probe for mp_crash (docs/research/cod4-combat.md 3.2),
// loaded over the map's own effects script. Every spawned player gets an
// AK-47 alone and no perk, its clip topped up every half second. The
// damage callback is wrapped: each round that reaches a player logs its
// point (in thousandths) and its direction (in millionths) and does no
// damage, so a `cod4_probe --script sway` shooter can fire at an idle
// target for as long as it likes. The lines go to games_mp.log.
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
