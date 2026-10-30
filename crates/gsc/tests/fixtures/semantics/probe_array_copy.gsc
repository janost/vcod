//	Is an array a value? Assign one to a second variable, write through
//	either, and read both sizes. CoD4's `_teams::CountPlayers` does
//	`players = level.players; players["allies"] = n;` and must leave
//	`level.players` alone. Run by tools/run_probe.sh; every logPrint line
//	is one measurement.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	a[0] = "x";
	b = a;
	b[1] = "y";
	logPrint("PROBE local_copy_write " + a.size + " " + b.size + "\n");

	c[0] = "x";
	d = c;
	c[1] = "y";
	logPrint("PROBE local_orig_write " + c.size + " " + d.size + "\n");

	level.parr[0] = "x";
	players = level.parr;
	players["allies"] = 1;
	logPrint("PROBE field_copy_strkey " + level.parr.size + " " + players.size + "\n");

	e[0] = "x";
	grow(e);
	logPrint("PROBE arg_write " + e.size + "\n");

	f = made();
	f[1] = "y";
	logPrint("PROBE returned_write " + f.size + "\n");

	g[0][0] = "x";
	h = g;
	h[0][1] = "y";
	logPrint("PROBE nested_write " + g[0].size + " " + h[0].size + "\n");

	k[0][0] = "x";
	m = k[0];
	m[1] = "y";
	logPrint("PROBE element_copy_write " + k[0].size + " " + m.size + "\n");

	level.q[0] = "x";
	level.r = level.q;
	level.r[1] = "y";
	logPrint("PROBE field_to_field " + level.q.size + " " + level.r.size + "\n");
}

grow(x)
{
	x[1] = "y";
}

made()
{
	n[0] = "x";
	return n;
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
