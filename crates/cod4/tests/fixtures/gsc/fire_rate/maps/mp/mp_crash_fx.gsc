// vcod's fire rate probe for mp_crash (docs/research/cod4-combat.md 2.1).
// Loaded over the map's own effects script under an fs_game: retail reads
// a mod's loose scripts first. Every spawned player gets an AK-47 alone,
// a full clip and no perk, Double Tap (specialty_rof) on top when the
// dvar vcod_rof is 1, so a cod4_probe holding attack (--script auto)
// fires an automatic weapon at the rate pmove gives it.
main()
{
	level thread fr_connect();
}

fr_connect()
{
	for ( ;; )
	{
		level waittill( "connected", player );
		player thread fr_spawn();
	}
}

fr_spawn()
{
	self endon( "disconnect" );
	for ( ;; )
	{
		self waittill( "spawned_player" );
		wait 0.05;
		self takeAllWeapons();
		self clearPerks();
		if ( getDvarInt( "vcod_rof" ) )
			self setPerk( "specialty_rof" );
		self giveWeapon( "ak47_mp" );
		self giveMaxAmmo( "ak47_mp" );
		self setSpawnWeapon( "ak47_mp" );
		logPrint( "PROBE " + getTime() + " armed rof " + getDvarInt( "vcod_rof" ) + "\n" );
	}
}
