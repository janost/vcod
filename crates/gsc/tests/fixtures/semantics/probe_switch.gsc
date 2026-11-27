//	`switch` keys: an int matches an int case, a string a string case, and
//	a float is fatal but on 1.5b. Run by tools/run_probe.sh; the float
//	switch is last.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE int_int " + sw_int(1) + "\n");
	logPrint("PROBE int_miss " + sw_int(3) + "\n");
	logPrint("PROBE numstr_int " + sw_int("1") + "\n");
	logPrint("PROBE int_str " + sw_str(1) + "\n");
	logPrint("PROBE str_str " + sw_str("1") + "\n");
	logPrint("PROBE str_upper " + sw_word("ABC") + "\n");
	logPrint("PROBE str_same " + sw_word("abc") + "\n");
	logPrint("PROBE at float\n");
	logPrint("PROBE float " + sw_int(1.0000001) + "\n");
	logPrint("PROBE done\n");
}

sw_int(v)
{
	r = "none";
	switch (v)
	{
	case 1:
		r = "one";
		break;
	case 2:
		r = "two";
		break;
	default:
		r = "default";
		break;
	}
	return r;
}

sw_str(v)
{
	r = "none";
	switch (v)
	{
	case "1":
		r = "one";
		break;
	default:
		r = "default";
		break;
	}
	return r;
}

sw_word(v)
{
	r = "none";
	switch (v)
	{
	case "abc":
		r = "abc";
		break;
	default:
		r = "default";
		break;
	}
	return r;
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
