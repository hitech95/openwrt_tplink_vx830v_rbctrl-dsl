/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Exercise OEM controller routines with a virtual DSP and fixture profile.
 * Scenarios controlled by environment variables:
 *   VOIP_CONCURRENT=1       - both lines active simultaneously
 *   VOIP_STARTUP_FAILURE=1  - withhold DSP-ready reply, then recover
 *   VOIP_PEER_INTERRUPT=1   - simulate peer disappearing during a call
 *   VOIP_BAD_DIGITS=1       - send invalid digit on line 0, expect no INVITE
 *   VOIP_TEARDOWN=1         - include on-hook teardown + channel release check
 */
typedef unsigned U;
#define F(a,t) ((t)(unsigned long)(a))
#define W(a) (*(volatile U *)(unsigned long)(a))
extern int sem_init(void *,int,unsigned),socketpair(int,int,int,int *);
extern int pthread_create(unsigned long *,const void *,void *(*)(void *),void *),pthread_join(unsigned long,void **);
extern long read(int,void *,unsigned long),write(int,const void *,unsigned long);
extern void _exit(int);
extern int strcmp(const char *,const char *);
extern char *strcpy(char *,const char *);
extern char *getenv(const char *);
struct pollfd { int fd; short events,revents; };
extern int close(int);
extern int dup(int),dup2(int,int);
extern int poll(struct pollfd *,unsigned long,int);
int util_execSystem(const char *where,const char *cmd,...) { return 0; }
static int count_present;
int system(const char *cmd) { count_present=cmd[0]=='e'; return 0; }
int access(const char *path,int mode) { return !strcmp(path,"/var/tmp/voip_cnxcount") && count_present ? 0 : -1; }
int dm_getObj(unsigned id,void *stack,unsigned size,void *out) { return -1; }
/* The offline fixture has no management-service call-history database. */
int rsl_clearXtpLastCallSession(U mask) { return 0; }
int rsl_addXtpLastCallSession(const void *entry) { return 0; }
int msg_send(void *handle,void *msg,...) { return -1; }
long sendto(int fd,const void *buf,unsigned long len,int flags,const void *addr,unsigned alen) { return -1; }
int open(const char *path,int flags,...) { return -1; }
static int dsp[2], sip[2], transcript;
static void provision(U id,const char *s) {
    char *p=F(0x407034,char *(*)(U,int,int))(id,-1,-1);
    if(!p) _exit(70);
    strcpy(p,s);
}
static void *startup(void *arg) {
    F(0x415070,void (*)(U))(7);
    F(0x412e3c,void (*)(U))(7);
    F(0x412e84,void (*)(U))(0);
    for (U i=0;i<4;i++) F(0x415170,void (*)(U))(i);
    return 0;
}
static void output(U cmd) {
    U rec[33];
    if(read(dsp[1],rec,sizeof(rec))!=132 || write(transcript,rec,132)!=132 || __builtin_bswap32(rec[0])!=cmd) _exit(80+cmd);
}
static void ingest(U cmd,U arg,U val) {
    U rec[33]={__builtin_bswap32(cmd),__builtin_bswap32(arg),__builtin_bswap32(val)};
    F(0x415e2c,void (*)(U *))(rec);
}
static void quiet(void) {
    struct pollfd p={dsp[1],1,0};
    if(poll(&p,1,50)!=0) _exit(75);
}
static void drain(void) {
    void *q=(void *)0x468070;
    U *m;
    while((m=F(0x404af0,U *(*)(void *))(q))) {
        F(0x418c18,void (*)(U *))(m);
        F(0x404788,void (*)(void *))(m);
    }
}
/* Read one local CMSIP record; the host checks its type-specific fields. */
static int read_sip(U *call) {
    struct pollfd s={sip[1],1,0};
    if(poll(&s,1,200)!=1) return -1;
    if(read(sip[1],call,816)!=816) return -2;
    return 0;
}
static void activate_media(U line) {
    U media[30]={0};
    media[4]=line; media[5]=3; media[6]=2; media[7]=8; media[8]=8;
    media[10]=20; media[11]=20; media[12]=1; media[13]=101; media[14]=101;
    media[15]=0x0100007f; media[16]=4000+2*line; media[17]=0x0100007f;
    media[18]=(5001+2*line)<<16 | (5000+2*line); media[29]=2;
    F(0x4165f8,void (*)(void *,U))(media,0xfffe); output(10);
    F(0x416be8,void (*)(void *))(media); output(12); output(91);
}
static void teardown_line(U line) {
    ingest(17,line,0); drain();
}
/* Drain all pending DSP records and write them to stdout. End with a zero record. */
static void drain_dsp(void) {
    struct pollfd out={dsp[1],1,0};
    U rec[33];
    for(U n=0;poll(&out,1,20)==1 && n<24;n++) {
        if(read(dsp[1],rec,132)!=132 || write(transcript,rec,132)!=132) _exit(64);
    }
    U end[33]={0}; if(write(transcript,end,132)!=132) _exit(65);
}
__attribute__((constructor)) static void run(void) {
    unsigned long th;
    int concurrent = getenv("VOIP_CONCURRENT") != 0;
    int startup_fail = getenv("VOIP_STARTUP_FAILURE") != 0;
    int peer_interrupt = getenv("VOIP_PEER_INTERRUPT") != 0;
    int bad_digits = getenv("VOIP_BAD_DIGITS") != 0;
    int teardown = getenv("VOIP_TEARDOWN") != 0;
    transcript=dup(1);
    if(transcript<0 || dup2(2,1)<0) _exit(63); /* OEM diagnostics are not records */
    F(0x4043ac,void (*)(void))();
    F(0x433d88,void (*)(void))();
    if(F(0x40497c,int (*)(void *))((void *)0x468070) || sem_init((void *)0x468050,0,0) ||
       socketpair(1,2,0,dsp) || socketpair(1,2,0,sip)) _exit(78);
    W(0x46006c)=dsp[0];
    W(0x460070)=sip[0];
    W(0x460068)=1;
    if(pthread_create(&th,0,startup,0)) _exit(79);
    output(1); quiet(); ingest(1,0,0);
    output(0); output(2);
    if(!startup_fail) {
        quiet(); ingest(2,0,0);
    } else {
        /* Withhold DSP-ready reply: verify no command-7 burst, then recover. */
        struct pollfd p={dsp[1],1,0};
        if(poll(&p,1,100)!=0) _exit(73); /* unexpected DSP record before ready reply */
        ingest(2,0,0); /* now supply the reply */
    }
    for(U i=0;i<4;i++) output(7);
    pthread_join(th,0);
    W(0x469978)=W(0x4699fc)=1;
    *(const char **)0x469988="fixture-user";
    *(const char **)0x4699d0="example.invalid";
    *(const char **)0x4601f0="127.0.0.1";
    W(0x4699d8)=5060; W(0x460058)=2; W(0x4602c0)=40;
    provision(0x2726,"5550100"); provision(0x2752,"1");
    if(F(0x438a34,int (*)(void))()) _exit(71);

    unsigned long ep[2]={0x46b518UL,0x46b518UL+0x408UL};
    for(U cid=0;cid<5;cid++) F(0x4275cc,void (*)(U))(cid);
    for(U line=0;line<2;line++) {
        W(ep[line]+0x34)=-1;
        W(ep[line]+0x38)=-1; /* no SIP call index until first call status */
    }

    /* ---- Off-hook both lines ---- */
    for(U line=0;line<2;line++) {
        ingest(18,line,0); drain(); output(8); output(13);
        ingest(24,line,'5'); drain();
        if(*(char *)(ep[line]+0x8c)!=0 || W(ep[line]+0x64)!=4) _exit(74);
        ingest(20,line,'5'); drain();
        output(13); /* first completed digit stops dial tone */
        U sample[3]={W(ep[line]+0x1c),W(ep[line]+0x64),W(ep[line]+0x34)};
        if(write(transcript,sample,sizeof(sample))!=sizeof(sample)) _exit(76);
    }

    /* ---- Send remaining digits; collect SIP INVITEs ---- */
    const char *rest[2]={"550100","550100"};
    if(bad_digits) {
        /* The invalid line must stay quiet; the valid line is a positive control. */
        ingest(20,0,'9'); drain();
        struct pollfd s={sip[1],1,0};
        if(poll(&s,1,300)!=0) _exit(77);
        while(*rest[1]) { ingest(20,1,*rest[1]++); drain(); }
        U call[204];
        if(read_sip(call) || F(0x41538c,U (*)(U))(call[2])!=1 ||
                write(transcript,call,816)!=816 || poll(&s,1,300)!=0) _exit(77);
        _exit(0);
    }
    U got_invite[2]={0,0};
    U invite_buf[2][204];
    U sip_count=0;

    while(sip_count<2) {
        struct pollfd s={sip[1],1,0};
        int pr=poll(&s,1,100);
        if(pr==1) {
            U call[204];
            if(read(sip[1],call,816)!=816) _exit(72);
            U seq=call[2];
            U line=seq-40;
            if(line>=2) _exit(73);
            if(got_invite[line]) _exit(74); /* duplicate INVITE for same line */
            for(U i=0;i<204;i++) invite_buf[line][i]=call[i];
            got_invite[line]=1;
            sip_count++;
            continue;
        }
        if(pr<0) _exit(75);
        /* timeout: send pending digits */
        for(U line=0;line<2;line++) {
            if(!got_invite[line] && *rest[line]) {
                ingest(20,line,*rest[line]++); drain();
            }
        }
        /* If both lines have no more digits and no INVITE yet, keep waiting briefly */
        U pending=0;
        for(U line=0;line<2;line++) if(!got_invite[line] && *rest[line]) pending=1;
        if(!pending && sip_count<2) {
            struct pollfd s2={sip[1],1,0};
            if(poll(&s2,1,300)!=1) {
                _exit(76); /* expected INVITE, none arrived */
            }
        }
    }

    /* ---- Write both INVITES to stdout, in line order ---- */
    for(U line=0;line<2;line++) {
        if(write(transcript,invite_buf[line],816)!=816) _exit(72);
    }

    /* ---- Verify INVITE contents: destination URI, sequence ---- */
    for(U line=0;line<2;line++) {
        U *call=invite_buf[line];
        if(call[2]!=40+line) _exit(78);
        char *uri=(char *)(call+4);
        if(strcmp(uri,"<sip:5550100@example.invalid:5060>")!=0) _exit(79);
    }

    /* Exercise the negotiated media serializers independently of SIP state. */
    for(U line=0;line<2;line++) activate_media(line);

    if(teardown || concurrent) {
        for(U line=0;line<2;line++) {
            U status[204]={1001,0,40+line,2+line,100};
            F(0x40fd08,void (*)(U *))(status); drain();
            if(W(ep[line]+0x38)!=2+line) _exit(83);
        }
    }


    /* ---- Verify both endpoints have DSP channels allocated ---- */
    for(U line=0;line<2;line++) {
        if(W(ep[line]+0x34)==-1) _exit(80);
    }

    /* ---- Peer interrupt: simulate peer disappearing during a call ---- */
    if(peer_interrupt) {
        /* No connected DSP descriptor: check failure before installing a new pair. */
        close(dsp[0]);
        close(dsp[1]);
        W(0x46006c)=-1;
        U slot=0;
        if(!F(0x4134c8,int (*)(U,void *,U))(7,&slot,4)) _exit(81);
        if(socketpair(1,2,0,dsp)) _exit(81);
        W(0x46006c)=dsp[0];
        /* Verify call state survived the interrupt */
        for(U line=0;line<2;line++) {
            if(W(ep[line]+0x34)==-1) _exit(82); /* channel lost during interrupt */
        }
    }

    /* ---- Teardown ---- */
    if(teardown || concurrent) {
        for(U line=0;line<2;line++) {
            teardown_line(line);
            if(concurrent && line==0 && (W(ep[1]+0x34)!=1 || W(ep[1]+0x38)!=3 ||
                    !W(0x460158+0x18))) _exit(68);
        }
        drain_dsp();
        for(U line=0;line<2;line++) {
            if(W(ep[line]+0x34)!=-1 || W(0x460158+0x18*line)!=0) _exit(66+line);
        }
        /* CMSIP CALLRELEASE requests (the SIP peer owns the network dialog). */
        for(U line=0;line<2;line++) {
            U bye[204];
            if(read_sip(bye)!=0) _exit(70+line);
            if(write(transcript,bye,816)!=816) _exit(72);
        }
    }
    _exit(0);
}
