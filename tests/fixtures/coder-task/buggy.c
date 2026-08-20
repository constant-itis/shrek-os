/* Freestanding: no libc, raw x86-64 syscalls, own _start. Built with `tcc -nostdlib -static`
 * (matches the sealed T2 rootfs — no libc in the guest). This is the coder-agent task fixture
 * (docs/phase6-slice2-coder-agent.md §5): it is SUPPOSED to print REAL-COMPILE-RUN-OK and exit 42,
 * but it does neither. The bounded task handed to the agent:
 *
 *     "Make the program print REAL-COMPILE-RUN-OK and exit 42."
 *
 * A real inspect -> edit -> build -> test -> return loop: the agent reads this file, writes a fix,
 * compiles it into /srv/build, runs it, and confirms the marker + exit code before finishing.
 */
static long s(long n, long a, long b, long c) {
    long r;
    __asm__ volatile("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c) : "rcx", "r11", "memory");
    return r;
}

void _start(void) {
    const char m[] = "WRONG-MARKER";   /* BUG: should be REAL-COMPILE-RUN-OK */
    s(1, 1, (long)m, sizeof(m) - 1);   /* write(stdout, m, len) */
    s(60, 0, 0, 0);                    /* BUG: exit(0), should be exit(42) */
}
