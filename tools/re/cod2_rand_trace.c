/* An LD_PRELOAD shim for retail cod2_lnxded 1.3 (md5
 * fefed4c4909fb7c5c13b13a003f5ccf7) that logs every srand() and rand() the
 * server makes to $RAND_TRACE_OUT, one line each:
 *
 *     srand <level.time> <call site> <seed>
 *     rand <level.time> <call site> <value>
 *
 * The call site is the `call rand@plt` instruction's address;
 * `level.time` is read from 0x864c56c, where G_RunFrame (0x810a13a) stores
 * it. cod2-combat.md 9.6 maps the sites. tools/run_probe_cod2.sh builds and
 * loads it with RAND_TRACE=1:
 *
 *     gcc -m32 -shared -fPIC -O2 -o cod2_rand_trace.so cod2_rand_trace.c -ldl
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static int out = -1;

static void say(const char *what, unsigned site, unsigned v)
{
	char line[96];
	int n;
	if (out < 0) {
		const char *path = getenv("RAND_TRACE_OUT");
		if (!path)
			return;
		out = open(path, O_WRONLY | O_CREAT | O_APPEND, 0644);
		if (out < 0)
			return;
	}
	n = snprintf(line, sizeof line, "%s %d %x %u\n", what,
		     *(volatile int *)0x864c56c, site, v);
	/* One write a line: the run script kills the server. */
	if (write(out, line, n) != n)
		out = -1;
}

void srand(unsigned seed)
{
	static void (*real)(unsigned);
	if (!real)
		real = (void (*)(unsigned))dlsym(RTLD_NEXT, "srand");
	say("srand", (unsigned)__builtin_return_address(0) - 5, seed);
	real(seed);
}

int rand(void)
{
	static int (*real)(void);
	int v;
	if (!real)
		real = (int (*)(void))dlsym(RTLD_NEXT, "rand");
	v = real();
	say("rand", (unsigned)__builtin_return_address(0) - 5, (unsigned)v);
	return v;
}
