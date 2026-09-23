//	What a handle to a deleted entity or a destroyed hudelem reads as, before
//	the deferred free, after it, and once a new spawn has taken the slot;
//	held in a local, in a field of level and in a field of an entity (the
//	sd.gsc progress-bar shape). Inside delete()'s 100 ms window the entity is
//	still live, so field reads, writes and methods run on it; past the free
//	only isDefined and == touch a stale handle here, since a field read, a
//	field write or a method call on one is fatal and each has its own probe.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at stale_setup\n");
}

Callback_StartGameType()
{
	logPrint("PROBE at stale_ent\n");
	e = spawn("script_origin", (0, 0, 0));
	e.targetname = "stale_a";
	n = e getEntityNumber();
	level.held_ent = e;
	e delete();
	logPrint("PROBE stale_ent_defined_immediate " + isdefined(e) + "\n");
	logPrint("PROBE stale_ent_level_defined_immediate " + isdefined(level.held_ent) + "\n");
	logPrint("PROBE stale_ent_window_read " + e.targetname + "\n");
	logPrint("PROBE stale_ent_window_number_same " + (e getEntityNumber() == n) + "\n");
	e.targetname = "stale_b";
	logPrint("PROBE stale_ent_window_write " + e.targetname + "\n");
	wait 0.15;
	logPrint("PROBE stale_ent_defined_after_free " + isdefined(e) + "\n");
	logPrint("PROBE stale_ent_level_defined_after_free " + isdefined(level.held_ent) + "\n");
	e2 = spawn("script_origin", (64, 0, 0));
	logPrint("PROBE stale_ent_slot_reused " + (e2 getEntityNumber() == n) + "\n");
	logPrint("PROBE stale_ent_defined_after_reuse " + isdefined(e) + "\n");
	logPrint("PROBE stale_ent_level_defined_after_reuse " + isdefined(level.held_ent) + "\n");
	logPrint("PROBE stale_ent_equals_new " + (e == e2) + "\n");
	logPrint("PROBE stale_ent_level_equals_new " + (level.held_ent == e2) + "\n");

	logPrint("PROBE at stale_hud\n");
	h = newHudElem();
	level.held_hud = h;
	h destroy();
	logPrint("PROBE stale_hud_defined_immediate " + isdefined(h) + "\n");
	logPrint("PROBE stale_hud_level_defined_immediate " + isdefined(level.held_hud) + "\n");
	h2 = newHudElem();
	logPrint("PROBE stale_hud_defined_after_new " + isdefined(h) + "\n");
	logPrint("PROBE stale_hud_level_defined_after_new " + isdefined(level.held_hud) + "\n");
	logPrint("PROBE stale_hud_equals_new " + (h == h2) + "\n");
	wait 0.15;
	logPrint("PROBE stale_hud_defined_after_wait " + isdefined(h) + "\n");

	logPrint("PROBE at stale_hud_in_ent_field\n");
	holder = spawn("script_origin", (128, 0, 0));
	holder.bar = newHudElem();
	holder.bar destroy();
	logPrint("PROBE stale_hud_ent_field_defined " + isdefined(holder.bar) + "\n");
	bar2 = newHudElem();
	logPrint("PROBE stale_hud_ent_field_defined_after_new " + isdefined(holder.bar) + "\n");
	logPrint("PROBE stale_hud_ent_field_equals_new " + (holder.bar == bar2) + "\n");

	logPrint("PROBE at stale_ent_equals_own_copy\n");
	logPrint("PROBE stale_ent_equals_own_copy " + (e == level.held_ent) + "\n");
	logPrint("PROBE stale_done\n");
}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
