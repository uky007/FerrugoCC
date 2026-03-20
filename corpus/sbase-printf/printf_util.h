/* Minimal subset of sbase util.h + utf.h for printf(1).
 * Skips regex.h, compat.h, and full libutf (no %b/%c rune handling). */

#ifndef PRINTF_UTIL_H
#define PRINTF_UTIL_H

#include <sys/types.h>
#include <stddef.h>
#include <stdio.h>

/* ── utf.h (minimal) ── */
typedef int Rune;

/* ── util.h subset ── */
extern char *argv0;

#undef MIN
#define MIN(x,y)  ((x) < (y) ? (x) : (y))

void *ecalloc(size_t, size_t);
void *emalloc(size_t);
void *erealloc(void *, size_t);
void *ereallocarray(void *, size_t, size_t);
char *estrdup(const char *);
char *estrndup(const char *, size_t);
void *encalloc(int, size_t, size_t);
void *enmalloc(int, size_t);
void *enrealloc(int, void *, size_t);
char *enstrdup(int, const char *);
char *enstrndup(int, const char *, size_t);

void enprintf(int, const char *, ...);
void eprintf(const char *, ...);
void weprintf(const char *, ...);
int  fshut(FILE *, const char *);

double estrtod(const char *);
long long estrtonum(const char *, long long, long long);
long long enstrtonum(int, const char *, long long, long long);

size_t unescape(char *);

/* UTF stubs for printf %c/%b (not fully implemented) */
size_t utflen(const char *);
void utftorunestr(const char *, Rune *);
void efputrune(Rune *, FILE *, const char *);

#endif
