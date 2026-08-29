//	Auto-vivification: does assigning to an index of an Undefined value turn
//	it into an array, and where does the new array get written back to?
//	Checkpointed: if the run dies, the last `at` names the killer.
//	Run by tools/run_probe.sh; every logPrint line is one measurement.
//	Groups are separate files because a script runtime error takes the whole
//	retail server down, so one fatal expression would cost every measurement
//	after it.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	maps\mp\gametypes\_callbacksetup::SetupCallbacks();

	logPrint("PROBE at viv_zero_index\n");
	a[0] = "x";
	logPrint("PROBE viv_zero_index_size " + a.size + "\n");
	logPrint("PROBE viv_zero_index_value " + a[0] + "\n");

	logPrint("PROBE at viv_nonzero_index\n");
	b[3] = "x";
	logPrint("PROBE viv_nonzero_index_size " + b.size + "\n");
	logPrint("PROBE viv_nonzero_index_value " + b[3] + "\n");

	logPrint("PROBE at viv_string_key\n");
	c["k"] = "v";
	logPrint("PROBE viv_string_key_size " + c.size + "\n");
	logPrint("PROBE viv_string_key_value " + c["k"] + "\n");

	logPrint("PROBE at add_to_array_shape\n");
	d = add_to_array(undefined, "ent1");
	logPrint("PROBE add_to_array_size " + d.size + "\n");
	logPrint("PROBE add_to_array_value " + d[0] + "\n");

	logPrint("PROBE at viv_field\n");
	level.myArray[0] = "y";
	logPrint("PROBE viv_field_size " + level.myArray.size + "\n");
	logPrint("PROBE viv_field_value " + level.myArray[0] + "\n");

	logPrint("PROBE at viv_non_array\n");
	e = 5;
	e[0] = "x";
	logPrint("PROBE viv_non_array_ok 1\n");
}

add_to_array(array, ent)
{
	if(!isdefined(ent))
		return array;
	if(!isdefined(array))
		array[0] = ent;
	else
		array[array.size] = ent;
	return array;
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
