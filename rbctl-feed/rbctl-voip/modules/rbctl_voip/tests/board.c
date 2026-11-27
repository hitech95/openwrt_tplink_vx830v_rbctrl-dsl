/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <stdio.h>
#include "board.h"

#define CHECK(e) do { if (!(e)) { fprintf(stderr, "%s failed\n", #e); return 1; } } while (0)

static int send_frame(void *ctx, const uint8_t *frame, size_t len)
{
	unsigned *calls = ctx;
	if (!frame || len < 14 || frame[12] != 0xdd || frame[13] != 1)
		return -EINVAL;
	++*calls;
	return -ENETDOWN;
}

int main(void)
{
	struct voice_board board = {0};
	const uint8_t mac[6] = {2, 0, 0, 0, 0, 1};
	unsigned calls = 0;

	CHECK(voice_board_bind(&board, mac, NULL, &calls) == EINVAL);
	for (unsigned i = 0; i < 2; ++i) {
		CHECK(voice_board_bind(&board, mac, send_frame, &calls) == 0);
		CHECK(voice_board_bind(&board, mac, send_frame, &calls) == EALREADY);
		CHECK(rbctl_voip_request(board.link, RBCTL_VOIP_SUB_INIT, NULL, 0, 0) == -ENETDOWN);
		CHECK(calls == i + 1);
		voice_board_close(&board);
		CHECK(board.link == NULL);
		voice_board_close(&board);
	}
	return 0;
}
