//	`scriptUsage` over threads whose locals, arrays, endons and frames are
//	known, under stock tdm on mp_brecourt (docs/research/cod11-server-handshake.md,
//	"Console commands over rcon"). The console command runs over rcon 4 s in.
//	No PROBE lines but `PROBE ready`: the capture is the command's reply.

main()
{
	maps\mp\gametypes\tdm::main();
	thread run();
}

run()
{
	wait 1;
	thread none();
	thread one();
	thread declared();
	thread unset();
	thread params(1);
	thread arr();
	thread shared();
	thread nested();
	thread endons();
	thread outer();
	thread twice();
	thread twice();
	thread vec();
	logPrint("PROBE ready\n");
}

none()
{
	wait 1000;
}

one()
{
	x = 1;
	wait 1000;
}

declared()
{
	x = 1;
	if (getcvar("nothing") == "x")
		y = 2;
	wait 1000;
}

unset()
{
	x = 1;
	y = undefined;
	wait 1000;
}

params(p, q)
{
	wait 1000;
}

arr()
{
	a = [];
	a[0] = 1;
	a[1] = 2;
	wait 1000;
}

shared()
{
	a = [];
	a[0] = 1;
	b = a;
	wait 1000;
}

nested()
{
	a = [];
	a[0] = [];
	a[0][0] = 1;
	a[0][1] = 2;
	wait 1000;
}

endons()
{
	self endon("a");
	level endon("b");
	level endon("c");
	wait 1000;
}

outer()
{
	x = 1;
	y = 2;
	inner(3);
}

inner(z)
{
	w = 4;
	wait 1000;
}

twice()
{
	s = "s";
	wait 1000;
}

vec()
{
	v = (1, 2, 3);
	s = "str";
	f = 1.5;
	wait 1000;
}
