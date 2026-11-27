/* Run the normal proxy main. Substitute only its platform resources.
 * Guest ABI: AArch64 Linux/musl. Test controller and L2 peers are local. */
extern void *dlsym(void *, const char *);
extern char *getenv(const char *);
extern int atoi(const char *), dup(int), close(int), unlink(const char *);
extern int open(const char *, int, ...), strcmp(const char *, const char *);
extern void *memcpy(void *, const void *, unsigned long);
extern int socketpair(int,int,int,int *);
extern long read(int,void *,unsigned long),write(int,const void *,unsigned long);
extern void _exit(int);
extern int *__errno_location(void);
#define NEXT(name) ((__typeof__(&name))dlsym((void *)-1L, #name))
static int raw[3] = {-1,-1,-1};
static int israw(int fd) { return fd==raw[0] || fd==raw[1] || fd==raw[2]; }
static int denied(void) { *__errno_location()=1; return -1; }
int socket(int af,int type,int proto) {
    if (af==17) {
        unsigned n=__builtin_bswap16((unsigned short)proto)-0xdd01;
        if(n>=3 || raw[n]>=0) return denied();
        const char *names[]={"DD01_FD","DD02_FD","DD03_FD"};
        return raw[n]=dup(atoi(getenv(names[n])));
    }
    if (af!=1 && af!=2) return denied();
    return NEXT(socket)(af,type,proto);
}
unsigned if_nametoindex(const char *s) { return !strcmp(s,"lan0.500") ? 1 : 0; }
int ioctl(int fd,unsigned long op,void *arg) {
    if(israw(fd) && op==0x8927) { memcpy((char*)arg+18,"\x02\0\0\0\0\x01",6);return 0; }
    return denied();
}
int setsockopt(int fd,int level,int opt,const void *v,unsigned len) {
    return israw(fd)?0:NEXT(setsockopt)(fd,level,opt,v,len);
}
int bind(int fd,const void *addr,unsigned len) {
    if(israw(fd)) return 0;
    unsigned short af=*(const unsigned short*)addr;
    if(af==1) { struct {unsigned short af;char path[8];} local={1,"dsp"};
        return NEXT(bind)(fd,&local,6); }
    if(af!=2 || ((const unsigned char*)addr)[4]!=127) return denied();
    return NEXT(bind)(fd,addr,len);
}
long sendto(int fd,const void *buf,unsigned long len,int flags,const void *addr,unsigned alen) {
    if(!addr || alen<16 || *(const unsigned short*)addr!=2 || ((const unsigned char*)addr)[4]!=127) return denied();
    return NEXT(sendto)(fd,buf,len,flags,addr,alen);
}
int unlink(const char *s) { return NEXT(unlink)(!strcmp(s,"/var/tmp/sock_CM_DSP")?"dsp":s); }
int access(const char *s,int mode) { return NEXT(access)(!strcmp(s,"/var/tmp/remote_init")?"init-marker":s,mode); }
int system(const char *s) {
    if(!strcmp(s,"echo 1 > /var/tmp/remote_init")) return close(open("init-marker",65,0600));
    if(!strcmp(s,"rm /var/tmp/remote_init -f")) return unlink("init-marker");
    return 0; /* suppress pid lookup and taskset shell commands */
}
void *fopen(const char *s,const char *mode) {
    if(!strcmp(s,"/var/tmp/voip_client.txt")) return 0;
    return NEXT(fopen)(s,mode);
}
int pthread_attr_setinheritsched(void *attr,int value) { return NEXT(pthread_attr_setinheritsched)(attr,0); }

/* Exercise the original client ring builder without the vendor management
 * daemon. Supply only its endpoint profile, a connected fd and call inputs. */
int util_execSystem(const char *where,const char *cmd,...) { return 0; } /* LEDs */
__attribute__((constructor)) static void client_records(void) {
    if (!getenv("PROBE_CLIENT")) return; /* proxy proceeds into original main */
    int pair[2]; unsigned char record[132];
    if(socketpair(1,2,0,pair)) _exit(80);
    *(int *)0x46006c = pair[0];
    for(unsigned line=0; line<2; ++line) {
        unsigned char *ep=(unsigned char *)(0x46b518UL+line*0x408);
        memcpy(ep+0x3c,"01020304",9);
        *(int *)(ep+0x78)=1<<line;
        ((void (*)(unsigned,unsigned,const char*,const char*,unsigned))0x412f6c)
            (line,0,"5550100","Fixture",0);
        for(int i=0;i<2;i++) {
            if(read(pair[1],record,sizeof(record))!=132 || write(1,record,sizeof(record))!=132) _exit(81);
        }
    }
    _exit(0);
}
