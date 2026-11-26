/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef RBCTL_PHONE_H
#define RBCTL_PHONE_H
#include <stdbool.h>
#include <rbctl_voip.h>
bool phone_board_ready(void);
unsigned phone_slot(void);
int phone_command(rbctl_voip_record rec);
int phone_play_packet(const uint8_t *data, size_t len);
void phone_hook(const rbctl_voip_record *event);
int phone_init(unsigned line);
void phone_close(void);
void phone_board_lost(void);
int phone_audio_init(void);
void phone_audio_close(void);
void phone_audio_mode(bool active, bool ringback);
void phone_audio_rx(const rbctl_voip_media *media);
#endif
