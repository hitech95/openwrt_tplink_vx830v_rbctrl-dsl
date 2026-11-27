/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef RBCTL_MODULE_BOARD_H
#define RBCTL_MODULE_BOARD_H

#include <rbctl_voip.h>

/* Owned by the libre event loop. Zero-initialize before first use. */
struct voice_board {
	rbctl_voip_link *link;
};

/* Attach a protocol engine to a caller-owned transport; positive errno on
 * error (baresip convention). Transport and ctx must outlive the link. */
int voice_board_bind(struct voice_board *board, const uint8_t mac[6],
		     rbctl_voip_send_fn send, void *ctx);
void voice_board_close(struct voice_board *board);

#endif
