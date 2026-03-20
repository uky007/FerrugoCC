/* sbase/wc corpus test driver — exercise wc's core counting + UTF-8 logic.
 *
 * Single-file compilation: include all sbase-wc .c files directly.
 * Tests line/word/char counting via pipes (no argv parsing).
 *
 * Note: uses output parameter instead of struct return (FerrugoCC
 * does not yet support struct return by value for structs > 8 bytes).
 */

#define _FORTIFY_SOURCE 0
#define _DONT_USE_CTYPE_INLINE_

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <errno.h>

#include "wc_util.h"

/* Inline eprintf/weprintf (same pattern as sbase-cat) */
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

void weprintf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    xvprintf(fmt, ap);
    va_end(ap);
}

/* Include sbase implementation files */
#include "runetype.c"
#include "rune.c"
#include "fgetrune.c"
#include "isspacerune.c"
#include "fshut.c"

/* wc core logic (extracted from wc.c, uses output parameter) */
static void do_wc(FILE *fp, const char *name, char cmode,
                  unsigned long *out_nc, unsigned long *out_nl, unsigned long *out_nw) {
    *out_nc = 0;
    *out_nl = 0;
    *out_nw = 0;

    int word = 0;
    int rlen;
    Rune c;

    while ((rlen = efgetrune(&c, fp, name))) {
        *out_nc += (cmode == 'c') ? rlen : (c != Runeerror);
        if (c == '\n')
            (*out_nl)++;
        if (!isspacerune(c))
            word = 1;
        else if (word) {
            word = 0;
            (*out_nw)++;
        }
    }
    if (word)
        (*out_nw)++;
}

static FILE *make_pipe_fp(const char *data, int len) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return NULL;
    write(pipefd[1], data, len);
    close(pipefd[1]);
    return fdopen(pipefd[0], "r");
}

static int test_wc_basic(void) {
    const char *text = "hello world\nfoo bar baz\n";
    FILE *fp = make_pipe_fp(text, strlen(text));
    if (!fp) return 1;

    unsigned long nc, nl, nw;
    do_wc(fp, "test", 'c', &nc, &nl, &nw);
    fclose(fp);

    if (nl != 2) return 2;
    if (nw != 5) return 3;
    if (nc != 24) return 4;
    return 0;
}

static int test_wc_empty(void) {
    FILE *fp = make_pipe_fp("", 0);
    if (!fp) return 10;

    unsigned long nc, nl, nw;
    do_wc(fp, "test", 'c', &nc, &nl, &nw);
    fclose(fp);

    if (nl != 0) return 11;
    if (nw != 0) return 12;
    if (nc != 0) return 13;
    return 0;
}

static int test_wc_single_line(void) {
    FILE *fp = make_pipe_fp("hello", 5);
    if (!fp) return 20;

    unsigned long nc, nl, nw;
    do_wc(fp, "test", 'c', &nc, &nl, &nw);
    fclose(fp);

    if (nl != 0) return 21;
    if (nw != 1) return 22;
    if (nc != 5) return 23;
    return 0;
}

static int test_wc_utf8(void) {
    /* "café\n" in UTF-8: c a f 0xc3 0xa9 \n = 6 bytes, 5 runes */
    const char text[7] = { 'c', 'a', 'f', '\xc3', '\xa9', '\n', '\0' };
    unsigned long nc, nl, nw;

    FILE *fp = make_pipe_fp(text, 6);
    if (!fp) return 30;
    do_wc(fp, "test", 'c', &nc, &nl, &nw);
    fclose(fp);
    if (nl != 1) return 31;
    if (nw != 1) return 32;
    if (nc != 6) return 33;

    fp = make_pipe_fp(text, 6);
    if (!fp) return 34;
    do_wc(fp, "test", 'm', &nc, &nl, &nw);
    fclose(fp);
    if (nl != 1) return 35;
    if (nw != 1) return 36;
    if (nc != 5) return 37;
    return 0;
}

static int test_wc_spaces(void) {
    const char *text = "  hello   world  \n";
    FILE *fp = make_pipe_fp(text, strlen(text));
    if (!fp) return 40;

    unsigned long nc, nl, nw;
    do_wc(fp, "test", 'c', &nc, &nl, &nw);
    fclose(fp);

    if (nl != 1) return 41;
    if (nw != 2) return 42;
    return 0;
}

int main(void) {
    int r;

    r = test_wc_basic();
    if (r != 0) return r;

    r = test_wc_empty();
    if (r != 0) return r;

    r = test_wc_single_line();
    if (r != 0) return r;

    r = test_wc_utf8();
    if (r != 0) return r;

    r = test_wc_spaces();
    if (r != 0) return r;

    return 42;
}
