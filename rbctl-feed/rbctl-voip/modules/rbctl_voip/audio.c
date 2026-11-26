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
static struct ausrc_st *src;
static struct auplay_st *play;
static struct ausrc *src_driver;
static struct auplay *play_driver;
static struct tmr clock_timer;
static bool enabled, ringing;
static bool board_clock, have_clock_packet;
static uint32_t clock_ssrc;
static uint16_t clock_seq;
static uint32_t timestamp, ssrc, ring_samples;
static uint16_t seq;
static uint64_t due_ms, max_late_ms, missed, emitted;

static void source_free(void *p) { if (src==p) src=NULL; }
static void player_free(void *p) { if (play==p) play=NULL; }
static int source_alloc(struct ausrc_st **st, const struct ausrc *as, struct ausrc_prm *prm,
		const char *dev, ausrc_read_h *read, ausrc_error_h *error, void *arg)
{
	(void)as; (void)dev; (void)error;
	if (src) return EBUSY;
	if (prm->srate!=8000 || prm->ch!=1 || prm->fmt!=AUFMT_S16LE || prm->ptime!=20) return ENOTSUP;
	src=mem_zalloc(sizeof(*src),source_free);
	if (!src) return ENOMEM;
	src->read=read; src->arg=arg; *st=src; return 0;
}
static int player_alloc(struct auplay_st **st, const struct auplay *ap, struct auplay_prm *prm,
		const char *dev, auplay_write_h *write, void *arg)
{
	(void)ap; (void)dev;
	if (play) return EBUSY;
	if (prm->srate!=8000 || prm->ch!=1 || prm->fmt!=AUFMT_S16LE || prm->ptime!=20) return ENOTSUP;
	play=mem_zalloc(sizeof(*play),player_free);
	if (!play) return ENOMEM;
	play->write=write; play->arg=arg; *st=play; return 0;
}

static void pull_and_send(void)
{
	int16_t pcm[NSAMP]={0};
	if (ringing) {
		/* Italy audible ringback: 425 Hz, 1 s on / 4 s off. */
		for (unsigned i=0;i<NSAMP;++i) {
			if (ring_samples<8000) pcm[i]=(int16_t)(4000*sin(ring_samples*0.3337942194439155));
			ring_samples=(ring_samples+1)%40000;
		}
	}
	else if (play) {
		struct auframe af;
		auframe_init(&af,AUFMT_S16LE,pcm,NSAMP,8000,1);
		play->write(&af,play->arg);
	}
	uint8_t packet[12+NSAMP]={0x80,8,seq>>8,seq,timestamp>>24,timestamp>>16,timestamp>>8,timestamp,
		ssrc>>24,ssrc>>16,ssrc>>8,ssrc};
	for (unsigned i=0;i<NSAMP;++i) packet[12+i]=g711_pcm2alaw(pcm[i]);
	++seq; timestamp+=NSAMP;
	int err=phone_play_packet(packet,sizeof(packet));
	if (!err) ++emitted;
	if (err) { warning("rbctl_voip: board audio send failed (%m)\n",err); phone_board_lost(); }
}

static void pump(void *arg)
{
	(void)arg;
	if (!enabled) return;
	uint64_t now = tmr_jiffies();
	uint64_t late = now > due_ms ? now - due_ms : 0;
	uint64_t skipped = late / PERIOD_MS;
	if (late > max_late_ms) max_late_ms = late;
	missed += skipped;
	timestamp += (uint32_t)(skipped * NSAMP);
	due_ms += (skipped + 1) * PERIOD_MS;
	tmr_start(&clock_timer,due_ms-now,pump,NULL);
	pull_and_send();
}

void phone_audio_mode(bool active, bool ringback)
{
	if (active && !enabled) {
		seq=rand_u16(); timestamp=rand_u32(); ssrc=rand_u32(); ring_samples=0;
		due_ms = tmr_jiffies() + PERIOD_MS;
		max_late_ms = missed = emitted = 0;
		have_clock_packet = false;
		if (!board_clock) tmr_start(&clock_timer,PERIOD_MS,pump,NULL);
	}
	if (!active) {
		tmr_cancel(&clock_timer);
		if (enabled) info("rbctl_voip: playout packets=%llu missed=%llu max_late_ms=%llu\n",
			(unsigned long long)emitted,(unsigned long long)missed,(unsigned long long)max_late_ms);
	}
	enabled=active; ringing=ringback;
}

void phone_audio_rx(const rbctl_voip_media *m)
{
	if (!enabled || m->slot!=phone_slot() || m->selector!=RBCTL_VOIP_RTP || m->len<12) return;
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
		if (have_clock_packet && id == clock_ssrc) {
			uint16_t delta = (uint16_t)(s-clock_seq);
			if (!delta || delta >= 0x8000) return;
			missed += delta-1;
			timestamp += (uint32_t)(delta-1)*NSAMP;
		}
		have_clock_packet = true; clock_seq = s; clock_ssrc = id;
		pull_and_send();
	}
	if (!enabled || !src) return;
	int16_t pcm[NSAMP]; struct auframe af;
	for (unsigned i=0;i<NSAMP;++i) pcm[i]=g711_alaw2pcm(p[off+i]);
	auframe_init(&af,AUFMT_S16LE,pcm,NSAMP,8000,1);
	af.timestamp=tmr_jiffies_usec();
	src->read(&af,src->arg);
}

int phone_audio_init(void)
{
	tmr_init(&clock_timer);
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
	phone_audio_mode(false,false);
	src_driver=mem_deref(src_driver); play_driver=mem_deref(play_driver);
}
