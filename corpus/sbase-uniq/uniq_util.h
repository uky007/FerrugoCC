/* Minimal subset of sbase text.h + util.h for uniq(1). */

#ifndef UNIQ_UTIL_H
#define UNIQ_UTIL_H

#include <sys/types.h>
#include <stddef.h>
#include <stdio.h>
#include <limits.h>

/* ── text.h ── */
struct line {
    char *data;
    size_t len;
};

/* ── util.h subset ── */
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
void *erealloc(void *, size_t);
int  fshut(FILE *, const char *);
long long estrtonum(const char *, long long, long long);

#endif
