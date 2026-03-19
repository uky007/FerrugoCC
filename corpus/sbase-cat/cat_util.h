/* Minimal subset of sbase util.h + arg.h for cat(1) only.
 * Removes regex.h, compat.h, and unused declarations. */

#ifndef CAT_UTIL_H
#define CAT_UTIL_H

#include <sys/types.h>
#include <stddef.h>
#include <stdio.h>

/* ── arg.h (inline) ── */

extern char *argv0;

#define ARGBEGIN	for (argv0 = *argv, argv++, argc--;\
				argv[0] && argv[0][0] == '-'\
				&& argv[0][1];\
				argc--, argv++) {\
			char argc_;\
			char **argv_;\
			int brk_;\
			if (argv[0][1] == '-' && argv[0][2] == '\0') {\
				argv++;\
				argc--;\
				break;\
			}\
			for (brk_ = 0, argv[0]++, argv_ = argv;\
					argv[0][0] && !brk_;\
					argv[0]++) {\
				if (argv_ != argv)\
					break;\
				argc_ = argv[0][0];\
				switch (argc_)

#define ARGEND		}\
		}

#define ARGC()		argc_

/* ── util.h subset ── */

void eprintf(const char *, ...);
void weprintf(const char *, ...);

ssize_t writeall(int, const void *, size_t);
int concat(int, const char *, int, const char *);

#endif
