/* Minimal subset of sbase utf.h + util.h + arg.h for wc(1) only. */

#ifndef WC_UTIL_H
#define WC_UTIL_H

#include <sys/types.h>
#include <stddef.h>
#include <stdio.h>

/* ── utf.h ── */

typedef int Rune;

enum {
    UTFmax    = 6,
    Runeself  = 0x80,
    Runeerror = 0xFFFD,
    Runemax   = 0x10FFFF
};

int charntorune(Rune *, const char *, size_t);
int chartorune(Rune *, const char *);
int runelen(Rune);
int fgetrune(Rune *, FILE *);
int efgetrune(Rune *, FILE *, const char *);
int isspacerune(Rune);

/* ── arg.h ── */

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
int  fshut(FILE *, const char *);

/* ── runetype.h ── */

#define nelem(x) (sizeof(x) / sizeof *(x))

int rune1cmp(const void *, const void *);
int rune2cmp(const void *, const void *);

#endif
