# Sourced by the scripts beside it: preflight checks that name what is
# missing and exit 2, before a run waits on something that will never come.

# need <command...>: each must be on PATH.
need() {
	local t
	for t; do
		command -v "$t" > /dev/null || { echo "${0##*/}: $t not found on PATH" >&2; exit 2; }
	done
}

# need_built <path> <how>: a binary the script runs, and the command that
# builds it.
need_built() {
	[[ -x $1 ]] || { echo "${0##*/}: $1 missing; build it with: $2" >&2; exit 2; }
}

# offlan_inner: refuse to run an off-LAN script's inner half outside the
# unshare its outer half starts.
offlan_inner() {
	if grep -qE '^ *0 +0 +4294967295$' /proc/self/uid_map; then
		echo "offlan: the inner half runs only inside its own unshare; unset OFFLAN_INNER" >&2
		exit 1
	fi
}

# offlan_veth: inside the off-LAN scripts' own net namespace, the server's
# side at 203.0.113.1 and a child net namespace at 198.51.100.2 (BPID, the
# pid of the sleep that holds it), joined by a veth pair. Neither address is
# private or shares three octets with the other, so no server sees a LAN.
offlan_veth() {
	ip link set lo up
	unshare -n sleep 3600 & BPID=$!
	sleep 0.5
	ip link add veth0 type veth peer name veth1 netns $BPID
	ip addr add 203.0.113.1/24 dev veth0; ip link set veth0 up
	ip route add 198.51.100.0/24 dev veth0
	nsenter -t $BPID -n sh -c 'ip link set lo up; ip addr add 198.51.100.2/24 dev veth1;
	    ip link set veth1 up; ip route add 203.0.113.0/24 dev veth1'
}
