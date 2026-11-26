/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <string.h>
#include "digitmap.h"

static uint16_t key(char c)
{
	if (c >= '0' && c <= '9') return 1u << (c-'0');
	return c == '*' ? 1u<<10 : c == '#' ? 1u<<11 : 0;
}

int digitmap_parse(struct digitmap *m, const char *s)
{
	memset(m,0,sizeof(*m));
	do {
		if (m->n == 16) return E2BIG;
		struct dial_rule *r = &m->rule[m->n++];
		while (*s && *s!='|' && *s!='T') {
			uint16_t mask=0;
			if (*s=='x' || *s=='X') { mask=1023; ++s; }
			else if (*s=='[') {
				++s;
				while (*s && *s!=']') {
					char lo=*s++;
					if (!key(lo)) return EINVAL;
					if (*s=='-') {
						++s; char hi=*s;
						if (lo<'0' || hi>'9' || hi<lo) return EINVAL;
						++s; for (char c=lo;c<=hi;++c) mask|=key(c);
					} else mask|=key(lo);
				}
				if (*s!=']') return EINVAL;
				++s;
			} else mask=key(*s++);
			if (!mask || r->n==32) return EINVAL;
			r->keys[r->n++]=mask;
			/* Terminal '.' = zero or more of previous atom; '+' = one or more. */
			if (*s=='.' || *s=='+') {
				r->repeat=true;
				if (*s++=='.') --r->n;
				else if (r->n<32) r->keys[r->n]=mask;
				break;
			}
		}
		if (!r->n && !r->repeat) return EINVAL;
		if (*s=='T') { r->timed=true; ++s; }
		if (!*s) return 0;
		if (*s++!='|') return EINVAL;
	} while (*s);
	return EINVAL;
}

enum dial_match digitmap_match(const struct digitmap *m, const char *digits)
{
	size_t n=strlen(digits); bool full=false, more=false, timed=false;
	for (unsigned i=0;i<m->n;++i) {
		const struct dial_rule *r=&m->rule[i];
		if (n>r->n && !r->repeat) continue;
		size_t j=0;
		for (;j<n;++j) {
			unsigned k=j<r->n ? (unsigned)j : (r->n<32 ? r->n : 31);
			if (!(key(digits[j]) & r->keys[k])) break;
		}
		if (j!=n) continue;
		if (n>=r->n && n) { full=true; timed|=r->timed; }
		more |= n<r->n || r->repeat;
	}
	return full ? (more || timed ? DIAL_POSSIBLE : DIAL_EXACT) : more ? DIAL_PREFIX : DIAL_INVALID;
}
