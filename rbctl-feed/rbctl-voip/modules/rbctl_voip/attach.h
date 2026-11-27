/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef RBCTL_ATTACH_H
#define RBCTL_ATTACH_H

/* One board owner, called only on the libre event-loop thread. */
int voice_attach_start(const char *device, unsigned locale);
void voice_attach_stop(void);

#endif
