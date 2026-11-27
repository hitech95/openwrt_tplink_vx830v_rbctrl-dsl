/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef RBCTL_MANAGE_H
#define RBCTL_MANAGE_H
const char *voice_board_state(void);
const char *voice_board_peer(void);
const char *voice_line_state(unsigned slot);
struct call *voice_call(unsigned slot, bool secondary);
int voice_control(unsigned slot, const char *uri);
int voice_reload(void);
bool voice_reload_pending(void);
#ifdef RBCTL_UBUS
int voice_bus_start(void);
void voice_bus_close(void);
void voice_changed(void);
#else
static inline int voice_bus_start(void) { return 0; }
static inline void voice_bus_close(void) {}
static inline void voice_changed(void) {}
#endif
#endif
