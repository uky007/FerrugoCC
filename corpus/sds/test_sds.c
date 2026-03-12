/* sds corpus test driver — exercise core SDS API and verify results.
 *
 * Avoids ctype.h inline functions (_DefaultRuneLocale needs GOT on macOS).
 * Provides local isspace/isdigit/isxdigit to bypass ctype.h.
 */

/* Custom ctype replacements to bypass macOS _DefaultRuneLocale GOT issues */
int isspace(int c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' ||
           c == '\f' || c == '\v';
}

int isdigit(int c) {
    return c >= '0' && c <= '9';
}

int isxdigit(int c) {
    return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') ||
           (c >= 'A' && c <= 'F');
}

int tolower(int c) {
    if (c >= 'A' && c <= 'Z') return c + 32;
    return c;
}

int toupper(int c) {
    if (c >= 'a' && c <= 'z') return c - 32;
    return c;
}

int isprint(int c) {
    return c >= 32 && c <= 126;
}

/* Disable ctype.h, assert, and macOS-specific header inlines */
#define _CTYPE_H_
#define NDEBUG
#define assert(x) ((void)0)
/* Disable _FORTIFY_SOURCE checked builtins (macOS replaces memcpy etc.) */
#define _FORTIFY_SOURCE 0
/* Prevent FD_SET macros (__darwin_check_fd_set_overflow needs GOT on macOS) */
#define _FD_SET

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>

/* Include implementation directly (single-file compilation). */
#include "sds.h"
#include "sdsalloc.h"
#include "sds.c"

int main(void)
{
    /* Test 1: sdsnew + sdslen */
    sds s = sdsnew("hello");
    if (s == 0)
        return 1;
    if (sdslen(s) != 5)
        return 2;

    /* Test 2: sdscat */
    s = sdscat(s, " world");
    if (sdslen(s) != 11)
        return 3;

    /* Test 3: sdscmp */
    sds s2 = sdsnew("hello world");
    if (sdscmp(s, s2) != 0)
        return 4;

    /* Test 4: sdsdup */
    sds s3 = sdsdup(s);
    if (sdscmp(s, s3) != 0)
        return 5;
    if (sdslen(s3) != 11)
        return 6;

    /* Test 5: sdscpy */
    s = sdscpy(s, "replaced");
    if (sdslen(s) != 8)
        return 7;

    /* Test 6: sdstrim */
    sds s4 = sdsnew("  trimmed  ");
    s4 = sdstrim(s4, " ");
    if (sdslen(s4) != 7)
        return 8;

    /* Test 7: sdsrange */
    sds s5 = sdsnew("Hello World");
    sdsrange(s5, 6, -1);
    if (sdslen(s5) != 5)
        return 9;

    /* Test 8: sdscatlen */
    sds s6 = sdsempty();
    s6 = sdscatlen(s6, "abc", 3);
    if (sdslen(s6) != 3)
        return 10;

    /* Test 9: sdsfromlonglong */
    sds s7 = sdsfromlonglong(12345);
    if (sdslen(s7) != 5)
        return 11;

    /* Test 10: sdsclear */
    sdsclear(s6);
    if (sdslen(s6) != 0)
        return 12;

    /* Cleanup */
    sdsfree(s);
    sdsfree(s2);
    sdsfree(s3);
    sdsfree(s4);
    sdsfree(s5);
    sdsfree(s6);
    sdsfree(s7);

    return 42;
}
