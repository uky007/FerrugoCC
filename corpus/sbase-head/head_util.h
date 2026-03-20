/* Minimal subset of sbase util.h + arg.h for head(1). */

#ifndef HEAD_UTIL_H
#define HEAD_UTIL_H

#include <sys/types.h>
#include <stddef.h>
#include <stdio.h>
#include <limits.h>

#undef MIN
#define MIN(x,y) ((x) < (y) ? (x) : (y))

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

#define ARGNUM		case '0':\
			case '1':\
			case '2':\
			case '3':\
			case '4':\
			case '5':\
			case '6':\
			case '7':\
			case '8':\
			case '9'

#define ARGEND		}\
		}

#define ARGC()		argc_

#define ARGNUMF()	(brk_ = 1, estrtonum(argv[0], 0, INT_MAX))

#define EARGF(x)	((argv[0][1] == '\0' && argv[1] == NULL)?\
			((x), abort(), (char *)0) :\
			(brk_ = 1, (argv[0][1] != '\0')?\
				(&argv[0][1]) :\
				(argc--, argv++, argv[0])))

void eprintf(const char *, ...);
void weprintf(const char *, ...);
int  fshut(FILE *, const char *);
long long estrtonum(const char *, long long, long long);

#endif
