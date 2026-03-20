/* sbase/printf corpus test driver — exercise format parsing + conversion.
 *
 * Single-file compilation: include sbase utility files directly.
 * Tests unescape, strtonum, estrtod, estrdup, format string building.
 * Skips %b/%c (requires full UTF rune handling).
 */

#define _FORTIFY_SOURCE 0
#define _DONT_USE_CTYPE_INLINE_

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <unistd.h>

#include "printf_util.h"

/* ── eprintf/weprintf (inline, same as other sbase corpora) ── */
char *argv0;

static void xvprintf(const char *fmt, va_list ap) {
    if (argv0 && strncmp(fmt, "usage", strlen("usage")))
        fprintf(stderr, "%s: ", argv0);
    vfprintf(stderr, fmt, ap);
    if (fmt[0] && fmt[strlen(fmt)-1] == ':') {
        fputc(' ', stderr);
        perror(NULL);
    }
}

void eprintf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    xvprintf(fmt, ap);
    va_end(ap);
    exit(1);
}

void enprintf(int status, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    xvprintf(fmt, ap);
    va_end(ap);
    exit(status);
}

void weprintf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    xvprintf(fmt, ap);
    va_end(ap);
}

int fshut(FILE *fp, const char *fname) {
    fflush(fp);
    if (ferror(fp)) return 1;
    return 0;
}

/* UTF stubs — not tested, just satisfy linker */
size_t utflen(const char *s) { return strlen(s); }
void utftorunestr(const char *s, Rune *r) { while (*s) *r++ = *s++; *r = 0; }
void efputrune(Rune *r, FILE *fp, const char *name) { while (*r) fputc(*r++, fp); }

/* Include sbase utility implementations */
#include "unescape.c"
#include "ealloc.c"
#include "strtonum.c"
#include "estrtod.c"

/* ── Tests ── */

static int test_unescape(void) {
    /* \n → newline */
    char buf1[16];
    strcpy(buf1, "hello\\nworld");
    size_t len = unescape(buf1);
    if (len != 11) return 1;
    if (buf1[5] != '\n') return 2;
    if (strcmp(buf1 + 6, "world") != 0) return 3;

    /* \t → tab */
    char buf2[16];
    strcpy(buf2, "a\\tb");
    len = unescape(buf2);
    if (len != 3) return 4;
    if (buf2[1] != '\t') return 5;

    /* \\ → backslash */
    char buf3[16];
    strcpy(buf3, "a\\\\b");
    len = unescape(buf3);
    if (len != 3) return 6;
    if (buf3[1] != '\\') return 7;

    /* no escapes → unchanged */
    char buf4[16];
    strcpy(buf4, "plain");
    len = unescape(buf4);
    if (len != 5) return 8;

    return 0;
}

static int test_strtonum(void) {
    const char *errstr;
    long long val;

    val = strtonum("42", 0, 100, &errstr);
    if (errstr != 0) return 10;
    if (val != 42) return 11;

    val = strtonum("0", -10, 10, &errstr);
    if (errstr != 0) return 12;
    if (val != 0) return 13;

    val = strtonum("-5", -10, 10, &errstr);
    if (errstr != 0) return 14;
    if (val != -5) return 15;

    /* out of range */
    val = strtonum("200", 0, 100, &errstr);
    if (errstr == 0) return 16;

    return 0;
}

static int test_estrdup(void) {
    char *s = estrdup("hello");
    if (strcmp(s, "hello") != 0) { free(s); return 20; }
    free(s);

    char *s2 = estrndup("hello world", 5);
    if (strcmp(s2, "hello") != 0) { free(s2); return 21; }
    free(s2);

    return 0;
}

static int test_format_build(void) {
    /* Test the pattern from printf.c: building format strings dynamically */
    /* printf("%*.*lld", width, precision, num) */
    char *fmt = estrdup("%*.*ll#");
    fmt[6] = 'd';  /* replace # with d */
    /* Verify the string */
    if (strcmp(fmt, "%*.*lld") != 0) { free(fmt); return 30; }

    /* Use it: printf to a buffer via snprintf */
    char buf[64];
    snprintf(buf, sizeof(buf), fmt, 0, -1, (long long)42);
    if (strcmp(buf, "42") != 0) { free(fmt); return 31; }
    free(fmt);

    /* With flag */
    fmt = estrdup("%#*.*ll#");
    fmt[1] = '-';  /* left-align flag */
    fmt[7] = 'x';  /* hex */
    char buf2[64];
    snprintf(buf2, sizeof(buf2), fmt, 10, -1, (long long)255);
    /* Should be "0xff      " or similar with left-align */
    /* Just check it starts with "ff" (without the # since we replaced it with -) */
    /* Actually %-*.*llx with width=10 → "ff        " */
    if (strncmp(buf2, "ff", 2) != 0) { free(fmt); return 32; }
    free(fmt);

    return 0;
}

static int test_estrtod(void) {
    double d = estrtod("3.14");
    /* Check approximately */
    if (d < 3.13 || d > 3.15) return 40;

    d = estrtod("0");
    if (d != 0.0) return 41;

    return 0;
}

int main(void) {
    int r;

    r = test_unescape();
    if (r != 0) return r;

    r = test_strtonum();
    if (r != 0) return r;

    r = test_estrdup();
    if (r != 0) return r;

    r = test_format_build();
    if (r != 0) return r;

    r = test_estrtod();
    if (r != 0) return r;

    return 42;
}
