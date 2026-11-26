//	CoD2 1.3's math builtins' roundings (cod2-gsc-diff.md 7): probe_math's
//	lines (cod11-gsc-object-model.md 9.3) and the cases CoD2's float sums,
//	libm calls and `fistp` split. Each value logs as its exact float,
//	`<24-bit mantissa>p<exponent>`, built with int() since CoD2 has no
//	`(int)` cast. Run by tools/run_probe_cod2_gsc.sh.

main()
{
	level.callbackStartGameType = ::Callback_StartGameType;
	level.callbackPlayerConnect = ::Callback_PlayerConnect;
	level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
	level.callbackPlayerDamage = ::Callback_PlayerDamage;
	level.callbackPlayerKilled = ::Callback_PlayerKilled;

	logPrint("PROBE dist_one_round " + bits(distance((0, 0, 0), (0, 1.5, 4096))) + "\n");
	logPrint("PROBE dist_diff_kept " + bits(distance((16777216, 3, 0), (-1, 0, 0))) + "\n");
	logPrint("PROBE dsq_one_round " + bits(distanceSquared((0, 0, 0), (4096, 1, 1))) + "\n");
	logPrint("PROBE dsq_diff_kept " + bits(distanceSquared((16777216, 0, 0), (-1, 0, 0))) + "\n");
	logPrint("PROBE len_one_round " + bits(length((0, 1.5, 4096))) + "\n");
	logPrint("PROBE len_big " + bits(length((0, 4096, 16777215))) + "\n");
	logPrint("PROBE norm_a " + vbits(vectorNormalize((0, 0, 1234.5))) + "\n");
	logPrint("PROBE norm_b " + vbits(vectorNormalize((0, 2, 8191))) + "\n");
	logPrint("PROBE norm_c " + vbits(vectorNormalize((3, -4, 12))) + "\n");
	logPrint("PROBE norm_zero " + vbits(vectorNormalize((0, 0, 0))) + "\n");
	logPrint("PROBE vta_a " + vbits(vectorToAngles((1, 1, 1))) + "\n");
	logPrint("PROBE vta_b " + vbits(vectorToAngles((3, -4, 5))) + "\n");
	logPrint("PROBE vta_c " + vbits(vectorToAngles((-1, 0.001, -0.25))) + "\n");
	logPrint("PROBE vta_d " + vbits(vectorToAngles((1740.5, -899.78, -143.4))) + "\n");
	logPrint("PROBE fwd_a " + vbits(anglesToForward((30, 45, 60))) + "\n");
	logPrint("PROBE right_a " + vbits(anglesToRight((30, 45, 60))) + "\n");
	logPrint("PROBE up_a " + vbits(anglesToUp((30, 45, 60))) + "\n");
	logPrint("PROBE fwd_b " + vbits(anglesToForward((12.34, 256.7, -33.3))) + "\n");
	logPrint("PROBE right_b " + vbits(anglesToRight((12.34, 256.7, -33.3))) + "\n");
	logPrint("PROBE up_b " + vbits(anglesToUp((12.34, 256.7, -33.3))) + "\n");
	logPrint("PROBE fwd_c " + vbits(anglesToForward((90, 180, 0))) + "\n");
	logPrint("PROBE sin_30 " + bits(sin(30)) + "\n");
	logPrint("PROBE sin_180 " + bits(sin(180)) + "\n");
	logPrint("PROBE sin_36000 " + bits(sin(36000)) + "\n");
	logPrint("PROBE cos_90 " + bits(cos(90)) + "\n");
	logPrint("PROBE cos_60 " + bits(cos(60)) + "\n");
	logPrint("PROBE sin_1e24 " + bits(sin(1000000.0 * 1000000.0 * 1000000.0 * 1000000.0)) + "\n");
	logPrint("PROBE tan_45 " + bits(tan(45)) + "\n");
	logPrint("PROBE tan_8999 " + bits(tan(89.99)) + "\n");
	logPrint("PROBE asin_small " + bits(asin(0.001)) + "\n");
	logPrint("PROBE asin_one " + bits(asin(1)) + "\n");
	logPrint("PROBE acos_small " + bits(acos(0.0005)) + "\n");
	logPrint("PROBE acos_neg " + bits(acos(-1)) + "\n");
	logPrint("PROBE atan_small " + bits(atan(0.006)) + "\n");
	logPrint("PROBE atan_one " + bits(atan(1)) + "\n");
	tiny = 0.0001 * 0.0001 * 0.0001 * 0.0001 * 0.0001;
	tinier = tiny * 0.001;
	logPrint("PROBE len_tiny " + bits(length((tiny, 0, 0))) + "\n");
	logPrint("PROBE len_tinier " + bits(length((tinier, 0, 0))) + "\n");
	logPrint("PROBE dist_tinier " + bits(distance((tinier, 0, 0), (0, 0, 0))) + "\n");
	logPrint("PROBE dsq_tiny " + bits(distanceSquared((tiny, 0, 0), (0, 0, 0))) + "\n");
	logPrint("PROBE norm_tiny " + vbits(vectorNormalize((tiny, 0, tiny))) + "\n");
	logPrint("PROBE norm_tinier " + vbits(vectorNormalize((tinier, 0, 0))) + "\n");
	logPrint("PROBE vta_flat_a " + vbits(vectorToAngles((4096, 1.5, 1))) + "\n");
	logPrint("PROBE vta_flat_b " + vbits(vectorToAngles((-4096, 1.5, -4096))) + "\n");
	logPrint("PROBE vta_tinier " + vbits(vectorToAngles((tinier, tinier, 1))) + "\n");
	logPrint("PROBE fwd_big " + vbits(anglesToForward((30, 36000.5, 0))) + "\n");
	logPrint("PROBE sin_small " + bits(sin(0.00001)) + "\n");
	logPrint("PROBE sin_neg " + bits(sin(-123.456)) + "\n");
	logPrint("PROBE cos_big " + bits(cos(1000000.0 * 1000000.0)) + "\n");
	logPrint("PROBE tan_90 " + bits(tan(90)) + "\n");
	logPrint("PROBE tan_big " + bits(tan(36000.5)) + "\n");
	logPrint("PROBE asin_neg " + bits(asin(-0.3)) + "\n");
	logPrint("PROBE acos_half " + bits(acos(0.5)) + "\n");
	logPrint("PROBE atan_big " + bits(atan(1000000.0 * 1000000.0)) + "\n");
	logPrint("PROBE int_neg " + int(-2.7) + "\n");
	logPrint("PROBE int_neg_half " + int(-0.5) + "\n");
	logPrint("PROBE int_top " + int(2147483520.0) + "\n");
	logPrint("PROBE int_huge " + int(3000000000.0) + "\n");
	logPrint("PROBE int_neg_huge " + int(-3000000000.0) + "\n");
	inf = 1000000.0 * 1000000.0 * 1000000.0 * 1000000.0;
	inf = inf * inf;
	logPrint("PROBE int_inf " + int(inf) + "\n");
	logPrint("PROBE int_nan " + int(inf - inf) + "\n");
	logPrint("PROBE math_done 1\n");
}

//	A float as its mantissa and exponent: halving and doubling are exact.
//	Retail's == has a tolerance, so zero is tested by order.
bits(f)
{
	s = "";
	if (f < 0)
	{
		s = "-";
		f = 0 - f;
	}
	else if (!(f > 0))
		return "0";
	e = 0;
	while (f >= 16777216)
	{
		f = f * 0.5;
		e = e + 1;
	}
	while (f < 8388608)
	{
		f = f * 2;
		e = e - 1;
	}
	return s + int(f) + "p" + e;
}

vbits(v)
{
	return bits(v[0]) + " " + bits(v[1]) + " " + bits(v[2]);
}

Callback_StartGameType() {}

Callback_PlayerConnect() {}
Callback_PlayerDisconnect() {}
Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {}
Callback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
