/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include "board.h"

int voice_board_bind(struct voice_board *board, const uint8_t mac[6],
		     rbctl_voip_send_fn send, void *ctx)
{
	uint32_t seed;
	int err;

	if (!board || !mac || !send)
		return EINVAL;
	if (board->link)
		return EALREADY;
	err = rbctl_voip_random_seed(&seed);
	if (err)
		return -err;
	board->link = rbctl_voip_link_new(mac, seed, send, ctx);
	return board->link ? 0 : ENOMEM;
}

void voice_board_close(struct voice_board *board)
{
	if (board) {
		rbctl_voip_link_free(board->link);
		board->link = NULL;
	}
}
