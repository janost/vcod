//	United Offensive's doDamage on a client and Team Deathmatch's killcam
//	skipped by use, for the A/B in crates/server/tests/gametypes_ab.rs. Runs
//	stock UO tdm.gsc on mp_cassino with two --save-scripted probes, allies
//	then axis, the damage and killed callbacks wrapped to log what the engine
//	hands them. The axis player takes four doDamage calls (an attacker and
//	no inflictor, no attacker with a fraction and no direction, an inflictor
//	apart from the attacker with a means of death, then the attacker's
//	kill), one each while dead and while in the killcam, and taps use 3.5 s
//	into the killcam. Then a doDamage with no attacker kills him again (no
//	killcam) and he taps use to respawn. Recipe and measurements:
//	docs/research/coduo-diff.md 22.

main()
{
	setCvar("scr_tdm_timelimit", "2");
	setCvar("scr_tdm_scorelimit", "0");
	thread choreograph();
	level thread place_players();
	maps\mp\gametypes\tdm::main();
	level.callbackPlayerDamage = ::probe_damage;
	level.callbackPlayerKilled = ::probe_killed;
	// Stock UO tdm reads it in Callback_PlayerKilled's world arm and never
	// sets it: a script runtime error, fatal at run_probe.sh's developer 1.
	level.teamkill_penalty = 0;
}

choreograph()
{
	if (getcvar("mapname") != "mp_cassino")
	{
		logPrint("PROBE unsupported " + getcvar("mapname") + "\n");
		return;
	}
	level thread watch_state();
	attacker = wait_player("allies");
	defender = wait_player("axis");
	logPrint("PROBE players " + attacker getEntityNumber() + " " + defender getEntityNumber() + "\n");
	wait 2;
	// A fixed source: the attacker's body creeps on the slope it lands on.
	src = home("allies");

	// An attacker and no inflictor: the inflictor is the attacker.
	defender doDamage(30, src, attacker);
	logPrint("PROBE back 1 " + getTime() + " " + defender.health + "\n");
	wait 1;
	// No attacker, a fraction, the source on the body (no direction).
	defender doDamage(10.6, defender.origin);
	logPrint("PROBE back 2 " + getTime() + " " + defender.health + "\n");
	wait 1;
	// An inflictor apart from the attacker, a means of death.
	defender doDamage(5, src + (0, 0, 100), attacker, defender, "MOD_PISTOL_BULLET");
	logPrint("PROBE back 3 " + getTime() + " " + defender.health + "\n");
	wait 1;
	defender doDamage(1000, src, attacker, attacker, "MOD_RIFLE_BULLET");
	logPrint("PROBE killed " + getTime() + " " + getTeamScore("allies") + " " + defender.sessionstate + "\n");
	wait 0.5;
	defender doDamage(10, src, attacker);
	logPrint("PROBE back dead " + getTime() + " " + defender.sessionstate + "\n");
	// The killcam starts 2 s after the kill.
	wait 2.5;
	defender doDamage(10, src, attacker);
	logPrint("PROBE back killcam " + getTime() + " " + defender.sessionstate + " " + defender.archivetime + "\n");
	// 3.5 s into the killcam: the tap ends it, and respawn's
	// waitRespawnButton reads the same press.
	wait 2.5;
	defender setClientCvar("probe_use", "tap");
	logPrint("PROBE tap " + getTime() + "\n");
	defender = wait_player("axis");
	defender setClientCvar("probe_use", "0");
	logPrint("PROBE respawned " + getTime() + "\n");
	wait 2;

	// No attacker: Callback_PlayerKilled's world arm, no killcam.
	defender doDamage(1000, defender.origin + (0, 0, -50));
	logPrint("PROBE world " + getTime() + " " + defender.sessionstate + "\n");
	wait 3;
	defender setClientCvar("probe_use", "tap");
	defender = wait_player("axis");
	defender setClientCvar("probe_use", "0");
	logPrint("PROBE respawned " + getTime() + "\n");
}

//	Logs what the engine hands the callback, then runs tdm's for a playing
//	player only: finishPlayerDamage on anyone else is not what is measured.
probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE damage " + getTime() + " " + self getEntityNumber() + " " + self.sessionstate + " hp=" + self.health + " inf=" + ent(eInflictor) + " att=" + ent(eAttacker) + " dmg=" + iDamage + " fl=" + iDFlags + " mod=" + sMeansOfDeath + " w=" + sWeapon + " pt=" + self point(vPoint) + " dir=" + str(vDir) + " loc=" + sHitLoc + "\n");
	if (self.sessionstate != "playing")
		return;
	self maps\mp\gametypes\tdm::Callback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
	logPrint("PROBE damaged " + getTime() + " " + self getEntityNumber() + " hp=" + self.health + "\n");
}

probe_killed(eInflictor, attacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc)
{
	logPrint("PROBE dying " + getTime() + " " + self getEntityNumber() + " inf=" + ent(eInflictor) + " att=" + ent(attacker) + " dmg=" + iDamage + " mod=" + sMeansOfDeath + " w=" + sWeapon + " dir=" + str(vDir) + " loc=" + sHitLoc + "\n");
	self maps\mp\gametypes\tdm::Callback_PlayerKilled(eInflictor, attacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc);
}

//	A source on the body reads "self": the body creeps on the slope it
//	landed on, by a different hundredth on each server.
point(v)
{
	if (isdefined(v) && distance(v, self.origin) == 0)
		return "self";
	return str(v);
}

ent(e)
{
	if (!isdefined(e))
		return "undef";
	return e getEntityNumber();
}

wait_player(team)
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			if (isdefined(p.pers["team"]) && p.pers["team"] == team && p.sessionstate == "playing")
				return p;
		}
		wait 0.05;
	}
}

//	One line whenever a watched value moves: the players' state, health,
//	score and deaths, the team scores and the game state.
watch_state()
{
	last = "";
	for (;;)
	{
		s = "";
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			p = players[i];
			s = s + " | " + p getEntityNumber() + " " + p.sessionstate + " hp=" + p.health + " sc=" + p.score + " d=" + p.deaths + " sp=" + p.spectatorclient;
		}
		s = s + " | allies " + getTeamScore("allies") + " axis " + getTeamScore("axis") + " " + str(game["state"]);
		if (s != last)
		{
			logPrint("PROBE state " + s + "\n");
			last = s;
		}
		wait 0.05;
	}
}

str(v)
{
	if (!isdefined(v))
		return "undef";
	return v;
}

//	Every spawn is moved onto a fixed spot per team, so what each probe sees
//	of the other is not the spawn script's random pick. A player already
//	playing when his watcher starts is placed at once: a spawn in the frame
//	before (a map_restart's) raises its "spawned" before anyone waits on it.
place_players()
{
	for (;;)
	{
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (!isdefined(players[i].probe_watch))
			{
				players[i].probe_watch = true;
				players[i] thread place_on_spawn();
			}
		}
		wait 0.05;
	}
}

place_on_spawn()
{
	if (self.sessionstate == "playing")
		self place();
	for (;;)
	{
		self waittill("spawned");
		wait 0.05;
		if (self.sessionstate == "playing")
			self place();
	}
}

place()
{
	self setOrigin(home(self.pers["team"]));
	self setPlayerAngles((0, 0, 0));
	logPrint("PROBE placed " + self getEntityNumber() + " " + self.pers["team"] + " " + self.origin + " " + self.angles + "\n");
}

home(team)
{
	return getentarray("mp_uo_spawn_" + team, "classname")[0].origin;
}
