/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <string.h>
#include <re.h>
#include <baresip.h>
#include "attach.h"
#include "board.h"
#include "phone.h"

#ifdef RBCTL_HARNESS
#include <fcntl.h>
#include <limits.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>

/* Duplicate the caller's inherited SOCK_DGRAM fd. Never own its original. */
static int fixture = -1;

static int fixture_open(void)
{
	const char *s = getenv("RBCTL_VOIP_TEST_FD");
	char *end;
	int type;
	socklen_t len = sizeof(type);
	if (!s || !*s)
		return EINVAL;
	errno = 0;
	long n = strtol(s, &end, 10);
	if (errno || *end || n < 0 || n > INT_MAX)
		return EINVAL;
	if (getsockopt((int)n, SOL_SOCKET, SO_TYPE, &type, &len))
		return errno;
	if (type != SOCK_DGRAM)
		return EPROTOTYPE;
	fixture = fcntl((int)n, F_DUPFD_CLOEXEC, 3);
	return fixture < 0 ? errno : 0;
}

static int fixture_send(void *ctx, const uint8_t *frame, size_t len)
{
	(void)ctx;
	/* Exercise synchronous transport failure before a command reaches the peer. */
	if (len >= 28 && frame[15] == 0x16 && !memcmp(frame+24,"\0\0\0\10",4)
	    && getenv("RBCTL_VOIP_TEST_FAIL_OPEN")) {
		unsetenv("RBCTL_VOIP_TEST_FAIL_OPEN");
		return -EIO;
	}
	ssize_t n = send(fixture, frame, len, MSG_DONTWAIT | MSG_NOSIGNAL);
	return n < 0 ? -errno : (size_t)n == len ? 0 : -EIO;
}
#endif

enum phase { DOWN, INIT, ATTACH, STARTUP, READY };
enum command { DSP_DEFAULT = 0, SLIC_LOCALE = 1, DSP_START = 2,
	CHANNEL_DEFAULT = 7, RTP_DEACTIVE = 12, CHANNEL_CLOSE = 31 };
struct step { enum command cmd; uint32_t arg; };
enum { SLOTS = 4 };
static struct {
	struct voice_board board;
	rbctl_voip_socket *socket;
	struct re_fhs *watch[3];
	struct tmr timer;
	uint64_t expires;
	enum phase phase;
	bool ack;
	int dsp_state;
	unsigned locale, cursor, count;
	struct step steps[2 * SLOTS + 3 + SLOTS];
	bool completed;
	rbctl_voip_record pending[24];
	unsigned front, queued;
	bool busy;
	unsigned inflight;
	unsigned inflight_slot;
	bool ring_maybe_on[PHONE_LINES], slot_maybe_open[PHONE_LINES];
} a;

/* Separate lifetime from an individual socket/link session. */
static struct {
	struct tmr timer;
	char device[16];
	unsigned locale;
	bool recovering;
} retry;
static int start_session(const char *device, unsigned locale);

static void close_session(void)
{
	/* On unload or loss of an ACK the peer may already have applied a
	 * ringing/open command. No event loop remains to await cleanup ACKs. */
	for (unsigned slot=0; a.phase==READY && slot<PHONE_LINES; ++slot) {
		if (!a.ring_maybe_on[slot] && !a.slot_maybe_open[slot]) continue;
		const unsigned stop[] = {6, 12, 31};
		for (size_t i = 0; i < sizeof(stop)/sizeof(stop[0]); ++i) {
			uint8_t raw[RBCTL_VOIP_RECORD_LEN];
			rbctl_voip_record rec = {.command=stop[i], .body.words={slot}};
			rbctl_voip_abandon(a.board.link);
			if (rbctl_voip_record_encode(raw,&rec,0) >= 0)
				(void)rbctl_voip_request(a.board.link,RBCTL_VOIP_SUB_RECORD,raw,sizeof(raw),tmr_jiffies());
		}
	}
	tmr_cancel(&a.timer);
	for (unsigned i = 0; i < 3; ++i)
		if (a.watch[i])
			a.watch[i] = fd_close(a.watch[i]);
	voice_board_close(&a.board);
	rbctl_voip_socket_free(a.socket);
#ifdef RBCTL_HARNESS
	if (fixture >= 0) {
		close(fixture);
		fixture = -1;
	}
#endif
	memset(&a, 0, sizeof(a));
}

void voice_attach_stop(void)
{
	tmr_cancel(&retry.timer);
	retry.recovering = false;
	close_session();
}

static void reconnect(void *unused)
{
	(void)unused;
	int err = start_session(retry.device, retry.locale);
	if (err) {
		warning("rbctl_voip: re-attach open failed (%m); retrying\n", err);
		tmr_start(&retry.timer, 2000, reconnect, NULL);
	}
}

static void fail(int err)
{
	warning("rbctl_voip: %s failed (%m)\n", a.phase == READY ? "call control" : a.phase == STARTUP ? "startup" : "attach", err);
	retry.recovering |= a.phase == READY;
	close_session();
	phone_board_lost();
	if (retry.recovering) tmr_start(&retry.timer, 1000, reconnect, NULL);
}

static void timer_handler(void *arg);

static void schedule(void)
{
	uint64_t deadline, next = a.expires, now = tmr_jiffies();
	if (a.phase == DOWN || (a.phase == READY && !a.busy))
		return;
	if (a.phase == READY) next = UINT64_MAX;
	if (rbctl_voip_deadline(a.board.link, &deadline) == 1 && deadline < next)
		next = deadline;
	tmr_start(&a.timer, next > now ? next-now : 0, timer_handler, NULL);
}

static void timer_handler(void *arg)
{
	(void)arg;
	uint64_t now = tmr_jiffies();
	if (a.phase != READY && now >= a.expires) {
		fail(ETIMEDOUT);
		return;
	}
	int rc = rbctl_voip_tick(a.board.link, now);
	if (rc < 0)
		fail(-rc);
	else
		schedule();
}

static void prepare_startup(void)
{
	if (!a.dsp_state) {
		for (unsigned i = 0; i < SLOTS; ++i) {
			a.steps[a.count++] = (struct step){ RTP_DEACTIVE, i };
			a.steps[a.count++] = (struct step){ CHANNEL_CLOSE, i };
		}
	}
	a.steps[a.count++] = (struct step){ SLIC_LOCALE, a.locale };
	a.steps[a.count++] = (struct step){ DSP_DEFAULT, a.locale };
	if (a.dsp_state)
		a.steps[a.count++] = (struct step){ DSP_START, 1 };
	for (unsigned i = 0; i < SLOTS; ++i)
		a.steps[a.count++] = (struct step){ CHANNEL_DEFAULT, i };
}

static int submit_step(void)
{
	if (tmr_jiffies() >= a.expires)
		return ETIMEDOUT;
	if (a.cursor == a.count) {
		a.phase = READY;
		retry.recovering = false;
		tmr_cancel(&a.timer);
		info("rbctl_voip: board ready (%s, locale %u)\n",
		     a.dsp_state ? "cold" : "warm", a.locale);
		return 0;
	}
	struct step step = a.steps[a.cursor];
	rbctl_voip_record rec = { .command = step.cmd, .body.words = {step.arg} };
	uint8_t raw[RBCTL_VOIP_RECORD_LEN];
	int rc = rbctl_voip_record_encode(raw, &rec, 0);
	if (rc < 0)
		return -rc;
	a.ack = false;
	a.completed = step.cmd != SLIC_LOCALE && step.cmd != DSP_START;
	return -rbctl_voip_request(a.board.link, RBCTL_VOIP_SUB_RECORD, raw,
				   sizeof(raw), tmr_jiffies());
}

bool phone_board_ready(void) { return a.phase == READY; }

static unsigned command_slot(const rbctl_voip_record *rec)
{
	return rec->command==10 || rec->command==11 ? rec->body.media.slot : rec->body.words[0];
}

static int dispatch(void)
{
	int first_error = 0;
	while (!a.busy && a.queued) {
		uint8_t raw[RBCTL_VOIP_RECORD_LEN];
		int rc = rbctl_voip_record_encode(raw, &a.pending[a.front], 0);
		a.inflight = a.pending[a.front].command;
		a.inflight_slot = command_slot(&a.pending[a.front]);
		if (rc >= 0)
			rc = rbctl_voip_request(a.board.link, RBCTL_VOIP_SUB_RECORD,
					       raw, sizeof(raw), tmr_jiffies());
		a.front = (a.front + 1) % 24;
		--a.queued;
		if (rc < 0) {
			warning("rbctl_voip: dropped command %u (%m)\n", a.inflight, -rc);
			if (!first_error) first_error = -rc;
			continue;
		}
		if (a.inflight == 5) a.ring_maybe_on[a.inflight_slot] = true;
		if (a.inflight == 8) a.slot_maybe_open[a.inflight_slot] = true;
		a.busy = true;
		schedule();
	}
	return first_error;
}

int phone_command(rbctl_voip_record rec)
{
	if (command_slot(&rec) >= PHONE_LINES) return EINVAL;
	if (!phone_board_ready()) return ENODEV;
	if (a.queued == 24) return ENOBUFS;
	a.pending[(a.front + a.queued++) % 24] = rec;
	return dispatch();
}

int phone_play_packet(unsigned slot, const uint8_t *data, size_t len)
{
	return phone_board_ready() ? -rbctl_voip_send_media(a.board.link, slot, RBCTL_VOIP_RTP, data, len) : ENODEV;
}

static int receive_frame(uint8_t *frame, size_t len)
{
	rbctl_voip_media media;
	int rc = rbctl_voip_receive(a.board.link, frame, len, &media);
	if (a.phase == READY && rc == RBCTL_VOIP_MEDIA) phone_audio_rx(&media);
	uint8_t raw[RBCTL_VOIP_RECORD_LEN];
	while (!rbctl_voip_pop(a.board.link, raw)) {
		rbctl_voip_record rec;
		if (rbctl_voip_record_decode(&rec, raw, sizeof(raw), 1))
			continue;
		if (a.phase == READY) phone_hook(&rec);
		if (a.phase == ATTACH && rec.command == DSP_START) {
			if (rec.body.words[0] > 1)
				return EPROTO;
			a.dsp_state = (int)rec.body.words[0];
		}
		else if (a.phase == STARTUP && rec.command == a.steps[a.cursor].cmd &&
			 (rec.command == SLIC_LOCALE || rec.command == DSP_START)) {
			if ((int32_t)rec.body.words[0] < 0 ||
			    (rec.command == DSP_START && rec.body.words[0] != 0))
				return EIO;
			a.completed = true;
		}
	}
	if (a.phase == READY && rc == -EREMOTEIO && a.busy) {
		unsigned cmd = a.inflight;
		warning("rbctl_voip: peer rejected command %u (status %u)\n", cmd,
			rbctl_voip_peer_status(a.board.link));
		a.busy = false;
		tmr_cancel(&a.timer);
		/* Unknown teardown state needs reconciliation. A rejected open,
		 * ring or activation invalidates dependent work for this call. */
		if (cmd == 6 || cmd == RTP_DEACTIVE || cmd == CHANNEL_CLOSE) return EREMOTEIO;
		if (cmd == 5 || cmd == 8 || cmd == 10) {
			unsigned failed = a.inflight_slot, keep = 0;
			for (unsigned i=0; i<a.queued; ++i) {
				rbctl_voip_record *r=&a.pending[(a.front+i)%24];
				if (command_slot(r) != failed)
					a.pending[(a.front+keep++)%24]=*r;
			}
			a.queued=keep;
			phone_line_lost(failed);
		}
		(void)dispatch();
		return 0;
	}
	if (rc < 0 && rc != -EAGAIN && rc != -EMSGSIZE && rc != -EBADMSG && rc != -EPROTO)
		return -rc;
	if (a.phase == INIT && rc == RBCTL_VOIP_CONTROL) {
		a.phase = ATTACH;
		return -rbctl_voip_request(a.board.link, RBCTL_VOIP_SUB_ATTACH, NULL, 0, tmr_jiffies());
	}
	if (a.phase == READY && rc == RBCTL_VOIP_CONTROL) {
		if (a.inflight == 6) a.ring_maybe_on[a.inflight_slot] = false;
		if (a.inflight == 31) a.slot_maybe_open[a.inflight_slot] = false;
		a.busy = false;
		tmr_cancel(&a.timer);
		(void)dispatch(); /* Individual errors are logged; the link remains usable. */
		return 0;
	}
	if (a.phase == ATTACH) {
		a.ack |= rc == RBCTL_VOIP_CONTROL;
		if (a.ack && a.dsp_state >= 0) {
			a.phase = STARTUP;
			info("rbctl_voip: attached (DSP %s)\n",
			     a.dsp_state ? "stopped" : "running");
			prepare_startup();
			return submit_step();
		}
	}
	else if (a.phase == STARTUP) {
		a.ack |= rc == RBCTL_VOIP_CONTROL;
		if (a.ack && a.completed) {
			++a.cursor;
			return submit_step();
		}
	}
	return 0;
}

static void readable(int flags, void *arg)
{
	(void)arg;
	if (flags & FD_EXCEPT) {
		fail(EIO);
		return;
	}
	/* Bound work per dispatch so SIP and timer callbacks get a turn. */
	for (unsigned i = 0; i < 32; ++i) {
		uint8_t frame[RBCTL_VOIP_FRAME_MAX];
		int n;
#ifdef RBCTL_HARNESS
		if (fixture >= 0) {
			n = (int)recv(fixture, frame, sizeof(frame), MSG_DONTWAIT | MSG_TRUNC);
			if (n > (int)sizeof(frame))
				continue;
			if (n < 0)
				n = errno == EAGAIN ? 0 : -errno;
		}
		else
#endif
			n = rbctl_voip_socket_recv(a.socket, frame);
		if (!n)
			break;
		int err = n < 0 ? -n : receive_frame(frame, (size_t)n);
		if (err) {
			fail(err);
			return;
		}
	}
	schedule();
}

static int start_session(const char *device, unsigned locale)
{
	uint8_t mac[6] = {2, 0, 0, 0, 0, 1};
	int fds[3], count = 3, err;
	rbctl_voip_send_fn send_fn = rbctl_voip_socket_send;
	if (a.phase != DOWN)
		return EALREADY;
	if (locale > 20)
		return EINVAL;
	a.locale = locale;
	tmr_init(&a.timer);
#ifdef RBCTL_HARNESS
	if (!strcmp(device, "fixture")) {
		err = fixture_open();
		fds[0] = fixture;
		count = 1;
		send_fn = fixture_send;
	}
	else
#endif
		err = -rbctl_voip_socket_open(device, &a.socket, mac, fds);
	if (!err)
		err = voice_board_bind(&a.board, mac, send_fn, a.socket);
	for (int i = 0; !err && i < count; ++i)
		err = fd_listen(&a.watch[i], fds[i], FD_READ, readable, NULL);
	if (!err) {
		a.phase = INIT;
		a.dsp_state = -1;
		a.expires = tmr_jiffies() + 5000;
		err = -rbctl_voip_request(a.board.link, RBCTL_VOIP_SUB_INIT, NULL, 0, tmr_jiffies());
	}
	if (err)
		close_session();
	else
		schedule();
	return err;
}

int voice_attach_start(const char *device, unsigned locale)
{
	if (!device || strlen(device) >= sizeof(retry.device)) return EINVAL;
	if (a.phase != DOWN || retry.recovering) return EALREADY;
	strcpy(retry.device, device);
	retry.locale = locale;
	tmr_init(&retry.timer);
	return start_session(device, locale);
}
