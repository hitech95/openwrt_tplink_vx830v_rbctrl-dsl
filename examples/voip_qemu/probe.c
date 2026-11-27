/* AArch64 musl constructor for the exact SHA-256 builds in run.py.
 * No firmware main, hardware, shell or AF_PACKET socket is used. */
typedef unsigned char u8;
typedef unsigned int u32;
typedef unsigned long usize;
extern int socketpair(int, int, int, int *);
extern long read(int, void *, usize), write(int, const void *, usize);
extern int close(int);
extern void _exit(int);
extern char *getenv(const char *);
extern void *memset(void *, int, usize);

#define FN(addr, type) ((type)(usize)(addr))
#define AT(addr, type) (*(type *)(usize)(addr))

static void emit(int fd, usize size) {
    u8 bytes[256];
    if (size > sizeof(bytes) || read(fd, bytes, sizeof(bytes)) != (long)size ||
        write(1, bytes, size) != (long)size) _exit(91);
}

__attribute__((constructor)) static void probe(void) {
    int pair[2];
    /* UNIX datagrams preserve the packets the proxy writes; the client
     * normally writes complete 132-byte records to a UNIX stream. */
    if (socketpair(1, 2, 0, pair)) _exit(90);
    if (getenv("PROBE_CLIENT")) {
        AT(0x46006c, int) = pair[0]; /* already connected DSP fd */
        for (u32 line = 0; line < 2; ++line) {
            FN(0x4133bc, void (*)(u32))(line); /* voip_stop_ring */
            emit(pair[1], 132);
        }
        FN(0x41511c, void (*)(void))(); /* default each media channel */
        for (int i = 0; i < 4; ++i) emit(pair[1], 132);
    } else {
        u8 request[0xbdc] __attribute__((aligned(16)));
        AT(0x41820c, int) = pair[0]; /* pre-opened outbound control fd */
        for (int i = 0; i < 2; ++i) {
            memset(request, 0, sizeof(request));
            if (FN(0x402028, int (*)(void *, int))(request, i ? 0x16 : 0x1a)) _exit(92);
            if (i) {
                /* Consume the OEM client's line-2 stop-ring record from stdin. */
                if (read(0, request + 24, 132) != 132) _exit(93);
            } else request[24] = 1;
            if (FN(0x402258, int (*)(void *, int))(request, i ? 132 : 4)) _exit(94);
            emit(pair[1], i ? 156 : 60);
        }
        /* Stock receive -> dispatch -> ACK. Repeat the identical normal
         * off-hook event; count queue entries, not just matching ACK bytes. */
        u8 event[156];
        if (read(0, event, sizeof(event)) != sizeof(event)) _exit(95);
        AT(0x418208, int) = pair[0];
        u8 *ctx = (u8 *)0x4182a0;
        ctx[12] = 0xdd; ctx[13] = 2; ctx[14] = 0x11; /* normally set at socket init */
        for (int i = 0; i < 2; ++i) {
            if (write(pair[1], event, sizeof(event)) != sizeof(event) ||
                FN(0x4026d4, int (*)(void *))(ctx) != 1 ||
                FN(0x401ed8, int (*)(void *))(ctx) != 0 || AT(0x418f60, int) != 1) _exit(96);
            emit(pair[1], 60);
        }
        AT(0x418224, int) = pair[0];
        FN(0x40337c, void (*)(void *))((void *)0x418f6c); /* deliver first queue slot */
        emit(pair[1], 132);

        /* Normal media wrapper; packet already supplied by synthetic UDP peer. */
        memset(request, 0, sizeof(request));
        *(int *)(request + 0x5ec) = pair[0];
        if (read(0, request + 24, 176) != 176 ||
            FN(0x403a78, int (*)(void *, int))(request, 176) != 0) _exit(97);
        emit(pair[1], 200);
    }
    _exit(0);
}
