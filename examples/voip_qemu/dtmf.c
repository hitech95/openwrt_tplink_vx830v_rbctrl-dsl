/* Test normal DSP event ingestion and CMSIP serialization at separate seams. */
typedef unsigned U;
#define CALL(a,t) ((t)(unsigned long)(a))
extern int sem_init(void *,int,unsigned),socketpair(int,int,int,int *);
extern long read(int,void *,unsigned long),write(int,const void *,unsigned long);
extern void _exit(int);
__attribute__((constructor)) static void dtmf(void) {
    int sip[2];
    void *q=(void *)0x468070;
    CALL(0x4043ac,void (*)(void))();
    if(CALL(0x40497c,int (*)(void *))(q) || sem_init((void *)0x468050,0,0) || socketpair(1,2,0,sip)) _exit(80);
    *(int *)0x460070=sip[0];
    for(unsigned line=0;line<2;line++) for(unsigned i=0;i<12;i++) {
        const char *digits="0123456789*#";
        for(unsigned k=0;k<3;k++) {
            unsigned cmd=(unsigned[]){24,20,71}[k];
            /* Hypothesis: local board digit is the ASCII key. Observe whether
             * the OEM dispatcher translates it; do not assume board output. */
            U rec[33]={__builtin_bswap32(cmd),__builtin_bswap32(line),__builtin_bswap32((U)digits[i])};
            CALL(0x415e2c,void (*)(U*))(rec);
            U *msg=CALL(0x404af0,U *(*)(void *))(q);
            if(!msg || write(1,msg,12)!=12) _exit(81);
            CALL(0x404788,void (*)(void *))(msg);
        }
        U out[6]={0x400003fa, line|((U)digits[i]<<16)};
        CALL(0x418c18,void (*)(U *))(out);
        unsigned char buf[816];
        if(read(sip[1],buf,sizeof(buf))!=816 || write(1,buf,16)!=16) _exit(82);
    }
    _exit(0);
}
