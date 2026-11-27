/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef RBCTL_PHONE_H
#define RBCTL_PHONE_H
#include <stdbool.h>
#include <rbctl_voip.h>
bool phone_board_ready(void);
const char *phone_panel_state(void);
enum { PHONE_LINES = 2 };
int phone_command(rbctl_voip_record rec);
int phone_play_packet(unsigned slot, const uint8_t *data, size_t len);
void phone_hook(const rbctl_voip_record *event);
int phone_init(unsigned line);
void phone_close(void);
void phone_board_lost(void);
void phone_line_lost(unsigned slot);
int phone_audio_init(unsigned default_line);
void phone_audio_close(void);
void phone_audio_mode(unsigned slot, bool active, bool ringback);
void phone_wait_beep(unsigned slot, bool start);
enum local_signal { SIGNAL_NONE, SIGNAL_HOWLER, SIGNAL_UNOBTAINABLE };
void phone_local_signal(unsigned slot, enum local_signal signal);
void phone_digit(unsigned slot, char digit);
void phone_fax(unsigned slot, bool active);
bool phone_detect_fax(unsigned slot);
uint64_t phone_rx_buffer_ms(unsigned slot);
void phone_audio_rx(const rbctl_voip_media *media);
#endif
