/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <math.h>
#include <string.h>
#include <re.h>
#include <rem.h>
#include <baresip.h>
#include "phone.h"

/* Single event-loop audio device. Baresip owns network RTP and its jitter
 * buffer. Board packets use an independent clock, sequence and SSRC. */
enum { NSAMP = 160, PERIOD_MS = 20 };
struct ausrc_st { ausrc_read_h *read; void *arg; };
struct auplay_st { auplay_write_h *write; void *arg; };
static struct ausrc *src_driver;
static struct auplay *play_driver;
struct leg {
	struct ausrc_st *src;
	struct auplay_st *play;
	struct tmr timer;
	unsigned slot;
	unsigned beep_left;
	unsigned ced_frames;
	enum local_signal signal;
	unsigned signal_pos, digit_pos;
	char digit;
	bool enabled, ringing, have_clock_packet;
	uint32_t clock_ssrc, timestamp, ssrc, ring_samples;
	uint16_t seq, clock_seq;
	uint64_t due_ms, max_late_ms, missed, emitted;
};
static struct leg legs[PHONE_LINES];
static bool board_clock;
static unsigned legacy_line;

static struct leg *device(const char *dev)
{
	if (!dev) return NULL;
	if (!strcmp(dev,"phone0")) return &legs[0];
	if (!strcmp(dev,"phone1")) return &legs[1];
	return !strcmp(dev,"phone") && legacy_line<PHONE_LINES ? &legs[legacy_line] : NULL;
}

static void source_free(void *p) { for (unsigned i=0;i<PHONE_LINES;++i) if (legs[i].src==p) legs[i].src=NULL; }
static void player_free(void *p) { for (unsigned i=0;i<PHONE_LINES;++i) if (legs[i].play==p) legs[i].play=NULL; }
static int source_alloc(struct ausrc_st **st, const struct ausrc *as, struct ausrc_prm *prm,
		const char *dev, ausrc_read_h *read, ausrc_error_h *error, void *arg)
{
	(void)as; (void)error;
	struct leg *l=device(dev);
	/* A sendonly re-INVITE may reopen a source on a held dialog. Give
	 * it an inert handle, never the active handset's microphone. */
	if (dev && !strcmp(dev,"held")) {
		*st=mem_zalloc(sizeof(**st),NULL);
		return *st ? 0 : ENOMEM;
	}
	if (!l) return ENODEV;
	if (l->src) return EBUSY;
	if (prm->srate!=8000 || prm->ch!=1 || prm->fmt!=AUFMT_S16LE || prm->ptime!=20) return ENOTSUP;
	struct ausrc_st *src=mem_zalloc(sizeof(*src),source_free);
	if (!src) return ENOMEM;
	src->read=read; src->arg=arg; *st=l->src=src; return 0;
}
static int player_alloc(struct auplay_st **st, const struct auplay *ap, struct auplay_prm *prm,
		const char *dev, auplay_write_h *write, void *arg)
{
	(void)ap;
	if (dev && !strcmp(dev,"held")) {
		*st=mem_zalloc(sizeof(**st),NULL);
		return *st ? 0 : ENOMEM;
	}
	struct leg *l=device(dev);
	if (!l) return ENODEV;
	if (l->play) return EBUSY;
	if (prm->srate!=8000 || prm->ch!=1 || prm->fmt!=AUFMT_S16LE || prm->ptime!=20) return ENOTSUP;
	struct auplay_st *play=mem_zalloc(sizeof(*play),player_free);
	if (!play) return ENOMEM;
	play->write=write; play->arg=arg; *st=l->play=play; return 0;
}

void phone_wait_beep(unsigned slot, bool start)
{
	legs[slot].beep_left=start ? 2400 : 0; /* 300 ms at 8 kHz, local RX only */
}

void phone_local_signal(unsigned slot, enum local_signal signal)
{
	legs[slot].signal=signal; legs[slot].signal_pos=0;
}

void phone_digit(unsigned slot, char digit)
{
	legs[slot].digit=digit; legs[slot].digit_pos=0;
}

static void pull_and_send(struct leg *l)
{
	int16_t pcm[NSAMP]={0};
	if (l->signal!=SIGNAL_NONE) {
		/* Conservative fixed-level warning and three-frequency SIT fallback. */
		for (unsigned i=0;i<NSAMP;++i) {
			unsigned n=l->signal_pos++, f=0;
			if (l->signal==SIGNAL_HOWLER) f=n%4000<2000 ? 1400 : 0;
			else { static const unsigned sit[]={950,1400,1800,0,0,0}; f=sit[(n/2640)%6]; }
			pcm[i]=(int16_t)(3500*sin((2*M_PI/8000)*f*n));
		}
	}
	else if (l->ringing) {
		/* Italy audible ringback: 425 Hz, 1 s on / 4 s off. */
		for (unsigned i=0;i<NSAMP;++i) {
			if (l->ring_samples<8000) pcm[i]=(int16_t)(4000*sin(l->ring_samples*0.3337942194439155));
			l->ring_samples=(l->ring_samples+1)%40000;
		}
	}
	else if (l->play) {
		struct auframe af;
		auframe_init(&af,AUFMT_S16LE,pcm,NSAMP,8000,1);
		l->play->write(&af,l->play->arg);
	}
	for (unsigned i=0;i<NSAMP && l->beep_left;++i,--l->beep_left)
		pcm[i]=(int16_t)(pcm[i]/2 + 4000*sin((2400-l->beep_left)*0.3337942194439155));
	uint16_t seq=l->seq++;
	uint32_t timestamp=l->timestamp, ssrc=l->ssrc;
	uint8_t packet[12+NSAMP]={0x80,8,seq>>8,seq,timestamp>>24,timestamp>>16,timestamp>>8,timestamp,
		ssrc>>24,ssrc>>16,ssrc>>8,ssrc};
	for (unsigned i=0;i<NSAMP;++i) packet[12+i]=g711_pcm2alaw(pcm[i]);
	l->timestamp+=NSAMP;
	int err=phone_play_packet(l->slot,packet,sizeof(packet));
	if (!err) ++l->emitted;
	if (!err && l->emitted%1500==0)
		info("rbctl_voip: slot %u packets=%llu rx_buffer_ms=%llu missed=%llu max_late_ms=%llu\n",
		     l->slot,(unsigned long long)l->emitted,(unsigned long long)phone_rx_buffer_ms(l->slot),
		     (unsigned long long)l->missed,(unsigned long long)l->max_late_ms);
	if (err) { warning("rbctl_voip: board audio send failed (%m)\n",err); phone_line_lost(l->slot); }
}

static void pump(void *arg)
{
	struct leg *l=arg;
	if (!l->enabled) return;
	uint64_t now = tmr_jiffies();
	uint64_t late = now > l->due_ms ? now - l->due_ms : 0;
	uint64_t skipped = late / PERIOD_MS;
	if (late > l->max_late_ms) l->max_late_ms = late;
	l->missed += skipped;
	l->timestamp += (uint32_t)(skipped * NSAMP);
	l->due_ms += (skipped + 1) * PERIOD_MS;
	tmr_start(&l->timer,l->due_ms-now,pump,l);
	pull_and_send(l);
}

void phone_audio_mode(unsigned slot, bool active, bool ringback)
{
	struct leg *l=&legs[slot];
	if (active && !l->enabled) {
		l->seq=rand_u16(); l->timestamp=rand_u32(); l->ssrc=rand_u32(); l->ring_samples=0;
		l->due_ms = tmr_jiffies() + PERIOD_MS;
		l->max_late_ms = l->missed = l->emitted = 0;
		l->have_clock_packet = false;
		if (!board_clock) tmr_start(&l->timer,PERIOD_MS,pump,l);
	}
	if (!active) {
		l->ced_frames=0;
		l->signal=SIGNAL_NONE; l->digit=0;
		l->beep_left=0;
		tmr_cancel(&l->timer);
		if (l->enabled) info("rbctl_voip: slot %u playout packets=%llu missed=%llu max_late_ms=%llu\n",
			slot,(unsigned long long)l->emitted,(unsigned long long)l->missed,(unsigned long long)l->max_late_ms);
	}
	l->enabled=active; l->ringing=ringback;
}

void phone_audio_rx(const rbctl_voip_media *m)
{
	if (m->slot>=PHONE_LINES || m->selector!=RBCTL_VOIP_RTP || m->len<12) return;
	struct leg *l=&legs[m->slot];
	if (!l->enabled) return;
	const uint8_t *p=m->datagram;
	if (p[0]>>6!=2 || (p[1]&127)!=8) return;
	size_t off=12+4*(p[0]&15), end=m->len;
	if (p[0]&16) {
		if (off+4>end) return;
		off+=4+4*((p[off+2]<<8)|p[off+3]);
	}
	if (off>end) return;
	if (p[0]&32) { unsigned pad=p[end-1]; if (!pad || pad>end-off) return; end-=pad; }
	if (end-off!=NSAMP) return;
	if (board_clock) {
		uint16_t s = ((uint16_t)p[2] << 8) | p[3];
		uint32_t id;
		memcpy(&id, p+8, sizeof(id));
		if (l->have_clock_packet && id == l->clock_ssrc) {
			uint16_t delta = (uint16_t)(s-l->clock_seq);
			if (!delta || delta >= 0x8000) return;
			l->missed += delta-1;
			l->timestamp += (uint32_t)(delta-1)*NSAMP;
		}
		l->have_clock_packet = true; l->clock_seq = s; l->clock_ssrc = id;
		pull_and_send(l);
	}
	if (!l->enabled || !l->src) return;
	int16_t pcm[NSAMP]; struct auframe af;
	for (unsigned i=0;i<NSAMP;++i) pcm[i]=g711_alaw2pcm(p[off+i]);
	/* 2100 Hz has exactly 42 cycles per 20-ms block. Require a dominant
	 * narrowband component for 500 ms before requesting G.711 passthrough. */
	if (phone_detect_fax(l->slot)) {
		double a=0,b=0,energy=0;
		for (unsigned i=0;i<NSAMP;++i) {
			double v=pcm[i]; energy+=v*v;
			double z=v+2*cos(2*M_PI*2100/8000)*a-b; b=a;a=z;
		}
		double power=a*a+b*b-2*cos(2*M_PI*2100/8000)*a*b;
		if (energy>NSAMP*200.*200 && power>.38*NSAMP*energy) {
			if (++l->ced_frames==25) phone_fax(l->slot,true);
		} else l->ced_frames=0;
	} else l->ced_frames=0;
	if (l->digit) {
		static const char keys[]="123456789*0#";
		static const unsigned row[]={697,770,852,941}, col[]={1209,1336,1477};
		const char *k=strchr(keys,l->digit);
		if (k) for (unsigned i=0;i<NSAMP;++i) {
			double t=(2*M_PI/8000)*l->digit_pos++;
			pcm[i]=(int16_t)(4800*sin(row[(k-keys)/3]*t)+4800*sin(col[(k-keys)%3]*t));
		}
		/* Bound a missing release edge; do not leak a stuck key into the call. */
		if (l->digit_pos>=5*8000) l->digit=0;
	}
	auframe_init(&af,AUFMT_S16LE,pcm,NSAMP,8000,1);
	af.timestamp=tmr_jiffies_usec();
	l->src->read(&af,l->src->arg);
}

int phone_audio_init(unsigned default_line)
{
	legacy_line=default_line;
	for (unsigned i=0;i<PHONE_LINES;++i) { legs[i].slot=i; tmr_init(&legs[i].timer); }
	char clock[16] = "timer";
	int err = conf_get_str(conf_cur(), "rbctl_voip_playout_clock", clock, sizeof(clock));
	if (err && err != ENOENT) return err;
	board_clock = !strcmp(clock, "board");
	if (!board_clock && strcmp(clock, "timer")) return EINVAL;
	err=ausrc_register(&src_driver,baresip_ausrcl(),"rbctl_voip",source_alloc);
	return err ? err : auplay_register(&play_driver,baresip_auplayl(),"rbctl_voip",player_alloc);
}
void phone_audio_close(void)
{
	for (unsigned i=0;i<PHONE_LINES;++i) phone_audio_mode(i,false,false);
	src_driver=mem_deref(src_driver); play_driver=mem_deref(play_driver);
}
