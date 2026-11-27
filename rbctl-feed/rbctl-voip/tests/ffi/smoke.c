// SPDX-License-Identifier: GPL-2.0-or-later
/* T2.15: the M3 embedding path. Uses only include/rbctl_voip.h and
 * librbctl_voip_ffi.a, exactly as the baresip module will. Built with
 * ASan/UBSan by crates/rbctl_voip_ffi/tests/c_smoke.rs.
 */
#include "rbctl_voip.h"
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>

static uint8_t last[RBCTL_VOIP_FRAME_MAX];
static size_t last_len;
static int sends;

static int capture(void *ctx, const uint8_t *frame, size_t len)
{
	assert(ctx == &sends);
	assert(len <= sizeof(last));
	memcpy(last, frame, len);
	last_len = len;
	sends++;
	return 0;
}

static const uint8_t host[6] = {2, 0, 0, 0, 0, 1};
static const uint8_t peer[6] = {2, 0, 0, 0, 0, 2};

/* Build a peer frame (marker 0x10) the way the Econet board does. */
static size_t peer_frame(uint8_t *f, uint16_t type, uint8_t sub, uint32_t seq,
			 const uint8_t *body, size_t n)
{
	memcpy(f, host, 6);
	memcpy(f + 6, peer, 6);
	f[12] = type >> 8;
	f[13] = type & 0xff;
	f[14] = 0x10;
	f[15] = sub;
	f[16] = seq >> 24;
	f[17] = seq >> 16;
	f[18] = seq >> 8;
	f[19] = seq;
	f[20] = n >> 8;
	f[21] = n & 0xff;
	f[22] = f[23] = 0;
	memcpy(f + 24, body, n);
	uint8_t covered[10 + 1514];
	memcpy(covered, f + 14, 10);
	memcpy(covered + 10, body, n);
	uint16_t crc = rbctl_voip_crc16(covered, 10 + n);
	f[22] = crc >> 8;
	f[23] = crc & 0xff;
	size_t len = 24 + n;
	if (type != RBCTL_VOIP_MEDIA && len < 60) {
		memset(f + len, 0, 60 - len);
		len = 60;
	}
	return len;
}

int main(void)
{
	assert(rbctl_voip_crc16((const uint8_t *)"123456789", 9) == 0xbb3d);
	assert(rbctl_voip_link_new(host, 0, NULL, NULL) == NULL);
	uint32_t seed;
	assert(rbctl_voip_random_seed(&seed) == 0);

	rbctl_voip_link *l = rbctl_voip_link_new(host, 0x7ffffffe, capture, &sends);
	assert(l);
	assert(rbctl_voip_request(l, 0x1c, NULL, 0, 0) == -EOPNOTSUPP);
	assert(rbctl_voip_request(NULL, RBCTL_VOIP_SUB_INIT, NULL, 0, 0) == -EINVAL);

	/* 1a init: golden vector req_init (broadcast, seq 0x7fffffff, CRC cc65). */
	assert(rbctl_voip_request(l, RBCTL_VOIP_SUB_INIT, NULL, 0, 0) == 0);
	assert(last_len == 60 && sends == 1);
	assert(!memcmp(last, "\xff\xff\xff\xff\xff\xff", 6));
	assert(last[14] == 0x11 && last[15] == 0x1a && last[22] == 0xcc && last[23] == 0x65);
	assert(rbctl_voip_request(l, RBCTL_VOIP_SUB_INIT, NULL, 0, 0) == -EBUSY);
	uint64_t due = 0;
	assert(rbctl_voip_deadline(l, &due) == 1 && due == 1000);
	assert(rbctl_voip_tick(l, 999) == 0 && sends == 1);
	assert(rbctl_voip_tick(l, 1000) == 0 && sends == 2);

	uint8_t f[RBCTL_VOIP_FRAME_MAX], zero[4] = {0};
	size_t n = peer_frame(f, RBCTL_VOIP_CONTROL, 0x1a, 0x7fffffff, zero, 4);
	assert(rbctl_voip_receive(l, f, n, NULL) == RBCTL_VOIP_CONTROL);
	assert(rbctl_voip_deadline(l, &due) == 0);

	/* DSP record codec: RTP_ACTIVE with the hardware-verified profile. */
	rbctl_voip_record r;
	memset(&r, 0, sizeof(r));
	r.command = 10;
	r.body.media.activation = 0xfffe;
	r.body.media.direction = 3;
	r.body.media.codec = 2;
	r.body.media.tx_pt = r.body.media.rx_pt = 8;
	r.body.media.tx_ms = r.body.media.rx_ms = 20;
	r.body.media.local_port = 4000;
	r.body.media.family = 2;
	uint8_t rec[RBCTL_VOIP_RECORD_LEN], again[RBCTL_VOIP_RECORD_LEN];
	assert(rbctl_voip_record_encode(rec, &r, 0) == RBCTL_VOIP_RECORD_LEN);
	assert(rec[3] == 10 && rec[4 + 0x18 + 3] == 2 && rec[4 + 0x14 + 3] == 3);
	assert(rec[4] == 0xff && rec[5] == 0xfe && rec[4 + 0x40] == 0x0f && rec[4 + 0x41] == 0xa0);
	assert(rec[4 + 0x77] == 2);
	rbctl_voip_record d;
	assert(rbctl_voip_record_decode(&d, rec, sizeof(rec), 0) == 0);
	assert(d.command == 10 && d.body.media.local_port == 4000);
	assert(rbctl_voip_record_encode(again, &d, 0) == RBCTL_VOIP_RECORD_LEN);
	assert(!memcmp(rec, again, sizeof(rec)));
	r.body.media.slot = 4;
	assert(rbctl_voip_record_encode(rec, &r, 0) == -EINVAL);
	r.command = 94;
	assert(rbctl_voip_record_encode(rec, &r, 0) == -EOPNOTSUPP);

	/* 16 request is unicast to the learned peer; nonzero status reported. */
	memset(&r, 0, sizeof(r));
	r.command = 6;
	r.body.words[0] = 1;
	assert(rbctl_voip_record_encode(rec, &r, 0) == RBCTL_VOIP_RECORD_LEN);
	assert(rbctl_voip_request(l, RBCTL_VOIP_SUB_RECORD, rec, sizeof(rec), 0) == 0);
	assert(!memcmp(last, peer, 6) && last_len == 156);
	uint8_t seven[4] = {0, 0, 0, 7};
	n = peer_frame(f, RBCTL_VOIP_CONTROL, 0x16, 0x80000000, seven, 4);
	assert(rbctl_voip_receive(l, f, n, NULL) == -EREMOTEIO && rbctl_voip_peer_status(l) == 7);

	/* DD02 off-hook on line 1: ACK status 0, one delivery, duplicate cached. */
	uint8_t ev[RBCTL_VOIP_RECORD_LEN] = {0, 0, 0, 18, 0, 0, 0, 1};
	n = peer_frame(f, RBCTL_VOIP_EVENT, 0x18, 9, ev, sizeof(ev));
	assert(rbctl_voip_receive(l, f, n, NULL) == RBCTL_VOIP_EVENT);
	uint8_t ack[60];
	assert(last_len == 60 && last[27] == 0);
	memcpy(ack, last, 60);
	assert(rbctl_voip_receive(l, f, n, NULL) == RBCTL_VOIP_EVENT && !memcmp(ack, last, 60));
	assert(rbctl_voip_pop(l, rec) == 0);
	assert(rbctl_voip_record_decode(&d, rec, sizeof(rec), 1) == 0);
	assert(d.command == 18 && d.body.words[0] == 1);
	assert(rbctl_voip_pop(l, rec) == -EAGAIN);

	/* DD03: view into our buffer, then unicast reply to the learned peer. */
	uint8_t media[] = {0, 2, 0, 4, 0x80, 0xc9, 0, 1};
	n = peer_frame(f, RBCTL_VOIP_MEDIA, 0x19, 0, media, sizeof(media));
	rbctl_voip_media m;
	assert(rbctl_voip_receive(l, f, n, &m) == RBCTL_VOIP_MEDIA);
	assert(m.slot == 2 && m.selector == RBCTL_VOIP_RTCP && m.len == 4 && m.datagram == f + 28);
	assert(rbctl_voip_send_media(l, 2, RBCTL_VOIP_RTCP, m.datagram, m.len) == 0);
	assert(last_len == 32 && !memcmp(last, peer, 6) && !memcmp(last + 24, media, 8));
	assert(rbctl_voip_send_media(l, 2, 5, m.datagram, m.len) == -EINVAL);

	rbctl_voip_link_free(l);
	rbctl_voip_link_free(NULL);
	puts("T2.15: C smoke test passed");
	return 0;
}
