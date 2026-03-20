/* See LICENSE file for copyright and license details. */
/* Modified: replaced designated initializer with switch (FerrugoCC compat) */
#include <ctype.h>
#include <string.h>

#include "printf_util.h"

#define is_odigit(c)  ('0' <= c && c <= '7')

static char escape_char(char c) {
	switch (c) {
	case '"':  return '"';
	case '\'': return '\'';
	case '\\': return '\\';
	case 'a':  return '\a';
	case 'b':  return '\b';
	case 'E':  return 033;
	case 'e':  return 033;
	case 'f':  return '\f';
	case 'n':  return '\n';
	case 'r':  return '\r';
	case 't':  return '\t';
	case 'v':  return '\v';
	default:   return 0;
	}
}

size_t
unescape(char *s)
{
	size_t m, q;
	char *r, *w;

	for (r = w = s; *r;) {
		if (*r != '\\') {
			*w++ = *r++;
			continue;
		}
		r++;
		if (!*r) {
			eprintf("null escape sequence\n");
		} else if (escape_char(*r)) {
			*w++ = escape_char(*r++);
		} else if (is_odigit(*r)) {
			for (q = 0, m = 4; m && is_odigit(*r); m--, r++)
				q = q * 8 + (*r - '0');
			*w++ = MIN(q, 255);
		} else if (*r == 'x' && isxdigit(r[1])) {
			r++;
			for (q = 0, m = 2; m && isxdigit(*r); m--, r++)
				if (isdigit(*r))
					q = q * 16 + (*r - '0');
				else
					q = q * 16 + (tolower(*r) - 'a' + 10);
			*w++ = q;
		} else {
			eprintf("invalid escape sequence '\\%c'\n", *r);
		}
	}
	*w = '\0';

	return w - s;
}
