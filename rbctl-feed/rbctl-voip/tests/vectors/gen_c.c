// SPDX-License-Identifier: GPL-2.0-or-later
/* Golden-vector generator for the M2.1 Rust port.
 *
 * Links the C prototype (voip-feed/vx830v-voip) and prints one
 * "name hex" line per case to stdout. The Rust test
 * crates/rbctl_voip_proto/tests/golden.rs rebuilds every case through the
 * Rust API and must match byte for byte. Inputs are fully deterministic.
 *
 * Build (see tests/vectors/README.md):
 *   cc -std=c11 -I<c>/include gen_c.c <c>/src/lib/{wire,dsp,link}.c -o gen
 *   ./gen > c_prototype.txt
 */
#include <vx/voice.h>
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>

static void hex(const char *name, const uint8_t *p, size_t n)
{
	printf("%s ", name);
	for (size_t i = 0; i < n; i++)
		printf("%02x", p[i]);
	putchar('\n');
}

static void rec(const char *name, struct vx_dsp d, int event)
{
	uint8_t b[VX_RECORD];

	assert(vx_dsp_encode(b, &d, event) == VX_RECORD);
	hex(name, b, sizeof(b));
}

/* Capture transport: remembers the last frame written. */
static uint8_t last[VX_FRAME];
static size_t last_len;
static int capture(void *ctx, const uint8_t *p, size_t n)
{
	(void)ctx;
	memcpy(last, p, n);
	last_len = n;
	return 0;
}

static const uint8_t host[6] = {2, 0, 0, 0, 0, 1};
static const uint8_t peer[6] = {2, 0, 0, 0, 0, 2};

static int inject(struct vx_link *l, unsigned int type, unsigned int sub,
		  uint32_t seq, const uint8_t *body, size_t n)
{
	struct vx_header h = {.type = type, .marker = VX_PEER, .sub = sub, .seq = seq};
	uint8_t f[VX_FRAME];

	memcpy(h.dst, host, 6);
	memcpy(h.src, peer, 6);
	int len = vx_frame_encode(f, sizeof(f), &h, body, n);
	assert(len > 0);
	return vx_receive(l, f, len, NULL);
}

static uint32_t lcg_state;
static uint8_t lcg(void)
{
	lcg_state = lcg_state * 1103515245u + 12345u;
	return (uint8_t)(lcg_state >> 16);
}

int main(void)
{
	/* 1. Semantic records as the CLI/controller build them. */
	for (unsigned int line = 0; line < 2; line++) {
		char n[64];
		struct vx_dsp cid = {.cmd = VX_CID,
				     .u.cid = {.line = line, .select = 1u << line,
					       .date = "01020304", .name = "Fixture",
					       .number = "5550100"}};
		snprintf(n, sizeof(n), "cid_line%u", line);
		rec(n, cid, 0);
		snprintf(n, sizeof(n), "ring_line%u", line);
		rec(n, (struct vx_dsp){.cmd = VX_RING,
				       .u.words = {line, 0, 0, UINT32_MAX, 1u << line, 0}}, 0);
		snprintf(n, sizeof(n), "open_slot%u", line);
		rec(n, (struct vx_dsp){.cmd = VX_CH_OPEN,
				       .u.words = {line, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0}}, 0);
		snprintf(n, sizeof(n), "rtp_active_slot%u", line);
		rec(n, (struct vx_dsp){.cmd = VX_RTP_ACTIVE,
				       .u.rtp = {.activation = 0xfffe, .slot = line, .direction = 3,
						 .codec = 2, .tx_pt = 8, .rx_pt = 8, .tx_ms = 20,
						 .rx_ms = 20, .dtmf = 1, .tx_event_pt = 101,
						 .rx_event_pt = 101, .local4 = {127, 0, 0, 1},
						 .remote4 = {127, 0, 0, 1},
						 .local_port = 4000 + 2 * line,
						 .remote_port = 5000 + 2 * line,
						 .control_port = 5001 + 2 * line, .family = 2}}, 0);
	}
	rec("rtp_update_v6", (struct vx_dsp){.cmd = VX_RTP_UPDATE,
		.u.rtp = {.slot = 3, .direction = 2, .codec = 3, .tos = 0xb8, .control_tos = 0x88,
			  .local6 = {0x20, 0x01, 0x0d, 0xb8, [15] = 1},
			  .remote6 = {0xfe, 0x80, [15] = 2}, .family = 10}}, 0);
	rec("tone_dial_line0", (struct vx_dsp){.cmd = VX_TONE, .u.tone.args = {0, 4, 0, 0}}, 0);
	rec("tone_stop_line1", (struct vx_dsp){.cmd = VX_TONE, .u.tone.args = {1, 0, 0, 0}}, 0);
	rec("tone_text", (struct vx_dsp){.cmd = VX_TONE,
		.u.tone = {.args = {1, 31, 0xffffffff, 1}, .text = {"Name", "123"}, .last = 9}}, 0);
	rec("slic_locale_10", (struct vx_dsp){.cmd = VX_LOCALE, .u.words = {10}}, 0);
	rec("dsp_start", (struct vx_dsp){.cmd = VX_START, .u.words = {1}}, 0);
	rec("mgmt70", (struct vx_dsp){.cmd = VX_MGMT, .u.words = {1, 2, 3, 4}}, 0);
	rec("dtmf_end_tx", (struct vx_dsp){.cmd = VX_DIGIT_END, .u.words = {1, '#', 1}}, 0);
	rec("dtmf_end_rx", (struct vx_dsp){.cmd = VX_DIGIT_END, .u.words = {1, '#', 1}}, 1);

	/* 2. Every command, both directions: LCG body -> C decode -> C encode. */
	const unsigned int ids[] = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
				    17, 18, 19, 20, 21, 22, 23, 24, 31, 70, 71, 72, 73, 74,
				    91, 92, 93};
	for (size_t i = 0; i < sizeof(ids) / sizeof(*ids); i++)
		for (int ev = 0; ev < 2; ev++) {
			uint8_t in[VX_RECORD], out[VX_RECORD];
			struct vx_dsp d;
			char n[64];

			lcg_state = ids[i] * 2u + (unsigned int)ev + 1;
			in[0] = in[1] = in[2] = 0;
			in[3] = (uint8_t)ids[i];
			for (size_t j = 4; j < sizeof(in); j++)
				in[j] = lcg();
			if (ids[i] == 10 || ids[i] == 11) { /* keep slot/family valid */
				memset(in + 20, 0, 4);
				in[23] = (uint8_t)(i & 3);
				memset(in + 120, 0, 4);
				in[123] = ev ? 2 : 10;
			}
			snprintf(n, sizeof(n), "in_%u_%s", ids[i], ev ? "rx" : "tx");
			hex(n, in, sizeof(in));
			assert(!vx_dsp_decode(&d, in, sizeof(in), ev));
			assert(vx_dsp_encode(out, &d, ev) == VX_RECORD);
			snprintf(n, sizeof(n), "norm_%u_%s", ids[i], ev ? "rx" : "tx");
			hex(n, out, sizeof(out));
		}

	/* 3. Link transactions. */
	struct vx_link l;
	uint8_t record[VX_RECORD];

	vx_link_init_seeded(&l, host, capture, NULL, 0x7ffffffe);
	assert(!vx_request(&l, 0x1a, NULL, 0, 0));
	hex("req_init", last, last_len);
	assert(!vx_tick(&l, 1000));
	hex("req_init_retry", last, last_len);
	uint8_t ok[4] = {0};

	assert(inject(&l, VX_CTL, 0x1a, l.seq, ok, 4) == VX_CTL);
	assert(!vx_request(&l, 0x1b, NULL, 0, 0));
	hex("req_attach_unicast", last, last_len);
	assert(inject(&l, VX_CTL, 0x1b, l.seq, ok, 4) == VX_CTL);
	struct vx_dsp stop = {.cmd = VX_RING_STOP, .u.words = {1}};

	assert(vx_dsp_encode(record, &stop, 0) == VX_RECORD);
	assert(!vx_request(&l, 0x16, record, sizeof(record), 0));
	hex("req_record", last, last_len);
	uint8_t st[4] = {0, 0, 0, 7};

	assert(inject(&l, VX_CTL, 0x16, l.seq, st, 4) == -EREMOTEIO && l.peer_status == 7);

	uint8_t ev[VX_RECORD] = {0, 0, 0, 18, 0, 0, 0, 1};

	assert(inject(&l, VX_EVENT, 0x18, 9, ev, sizeof(ev)) == VX_EVENT);
	hex("ack_ok_seq9", last, last_len);
	assert(inject(&l, VX_EVENT, 0x18, 9, ev, sizeof(ev)) == VX_EVENT);
	hex("ack_dup_seq9", last, last_len);
	assert(inject(&l, VX_EVENT, 0x18, 10, ev, 131) == VX_EVENT);
	hex("ack_short_seq10", last, last_len);
	for (uint32_t s = 11; s < 11 + 14; s++)
		assert(inject(&l, VX_EVENT, 0x18, s, ev, sizeof(ev)) == VX_EVENT);
	assert(inject(&l, VX_EVENT, 0x18, 25, ev, sizeof(ev)) == VX_EVENT);
	hex("ack_full_seq25", last, last_len);

	uint8_t rtp[172] = {128, 8, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4};

	memset(rtp + 12, 0xd5, 160);
	assert(!vx_send_media(&l, 1, 3, rtp, sizeof(rtp)));
	hex("media_tx_broadcast", last, last_len);
	uint8_t mbody[] = {0, 2, 0, 4, 0x80, 0xc9, 0, 1};

	assert(inject(&l, VX_MEDIA, 0x19, 0, mbody, sizeof(mbody)) == VX_MEDIA);
	assert(!vx_send_media(&l, 2, 4, mbody + 4, 4));
	hex("media_tx_learned", last, last_len);
	return 0;
}
