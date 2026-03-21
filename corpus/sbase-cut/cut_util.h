/* Minimal subset of sbase text.h + utf.h + util.h for cut(1). */

#ifndef CUT_UTIL_H
#define CUT_UTIL_H

#include <sys/types.h>
#include <stddef.h>
#include <stdio.h>
#include <limits.h>

/* ── text.h ── */
struct line {
    char *data;
    size_t len;
};

/* ── utf.h (minimal) ── */
#define UTF8_POINT(c) (((c) & 0xc0) != 0x80)

int fullrune(const char *, size_t);
int charntorune(int *, const char *, size_t);
int runelen(int);

/* ── util.h subset ── */
#undef MIN
#define MIN(x,y) ((x) < (y) ? (x) : (y))
#undef MAX
#define MAX(x,y) ((x) > (y) ? (x) : (y))

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

#define EARGF(x)	((argv[0][1] == '\0' && argv[1] == NULL)?\
			((x), abort(), (char *)0) :\
			(brk_ = 1, (argv[0][1] != '\0')?\
				(&argv[0][1]) :\
				(argc--, argv++, argv[0])))

void eprintf(const char *, ...);
void weprintf(const char *, ...);
int  fshut(FILE *, const char *);
void *ereallocarray(void *, size_t, size_t);
size_t unescape(char *);

#undef memmem
#define memmem xmemmem
void *memmem(const void *, size_t, const void *, size_t);

#endif
