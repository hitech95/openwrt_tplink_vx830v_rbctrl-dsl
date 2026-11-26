/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef RBCTL_DIGITMAP_H
#define RBCTL_DIGITMAP_H
#include <stdbool.h>
#include <stdint.h>

/* Alternatives are bounded by the config length, and each alternative by the
 * 32-digit collection buffer. No backtracking on the event-loop thread. */
struct dial_rule { uint16_t keys[32]; unsigned n; bool repeat, timed; };
struct digitmap { struct dial_rule rule[16]; unsigned n; };
enum dial_match { DIAL_INVALID, DIAL_PREFIX, DIAL_POSSIBLE, DIAL_EXACT };
int digitmap_parse(struct digitmap *m, const char *text);
enum dial_match digitmap_match(const struct digitmap *m, const char *digits);
#endif
