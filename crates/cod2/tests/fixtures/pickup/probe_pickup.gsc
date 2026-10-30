// A use-key weapon swap (cod2-combat.md 6.3): every client spawns 40 units
// behind the misc_turret nearest probe_x/y/z facing along it, holding a
// kar98k with a thompson in the other slot, and an mp40 hangs 30 units
// ahead of him at eye height. The use press drops the kar98k where the
// mp40 lay and selects the mp40 (`a <index>`).
//   PROBE_GSC=crates/cod2/tests/fixtures/pickup/probe_pickup.gsc \
//   tools/run_probe_cod2.sh pickup mp_carentan 1718 1824 5 0 <out>
main()
{
	level.callbackStartGameType = ::startGameType;
	level.callbackPlayerConnect = ::playerConnect;
	level.callbackPlayerDisconnect = ::nothing;
	level.callbackPlayerDamage = ::nothing;
	level.callbackPlayerKilled = ::nothing;
	maps\mp\gametypes\_callbacksetup::SetupCallbacks();
}

startGameType()
{
	precacheItem("kar98k_mp");
	precacheItem("thompson_mp");
	precacheItem("mp40_mp");
	game["state"] = "playing";
}

nearestTurret(at)
{
	best = undefined;
	guns = getEntArray("misc_turret", "classname");
	for(i = 0; i < guns.size; i++)
	{
		if(!isDefined(best) || distance(guns[i].origin, at) < distance(best.origin, at))
			best = guns[i];
	}
	return best;
}

playerConnect()
{
	self waittill("begin");
	self.sessionteam = "none";
	self.sessionstate = "playing";
	self.spectatorclient = -1;
	self.archivetime = 0;
	self.psoffsettime = 0;
	self.statusicon = "";
	self.maxhealth = 100;
	self.health = 100;
	at = (getCvarFloat("probe_x"), getCvarFloat("probe_y"), getCvarFloat("probe_z"));
	gun = nearestTurret(at);
	spot = gun.origin - vectorScale(anglesToForward((0, gun.angles[1], 0)), 40);
	trace = bulletTrace(spot + (0, 0, 16), spot - (0, 0, 256), false, undefined);
	origin = trace["position"] + (0, 0, 0.125);
	self spawn(origin, (0, gun.angles[1], 0));
	self giveWeapon("kar98k_mp");
	self giveWeapon("thompson_mp");
	self setSpawnWeapon("kar98k_mp");
	logPrint("PROBE spawned " + self.origin + "\n");
	pos = self.origin + (0, 0, 60) + vectorScale(anglesToForward((0, gun.angles[1], 0)), 30);
	item = spawn("weapon_mp40_mp", pos, 1);
	logPrint("PROBE item " + item getEntityNumber() + " " + item.origin + "\n");
	self thread watchItem(item);
}

vectorScale(v, s)
{
	return (v[0] * s, v[1] * s, v[2] * s);
}

watchItem(item)
{
	item waittill("trigger", who, dropped);
	logPrint("PROBE trigger " + who getEntityNumber() + " " + isDefined(dropped) + "\n");
}

nothing()
{
}
