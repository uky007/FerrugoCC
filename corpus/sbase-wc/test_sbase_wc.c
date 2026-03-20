/* sbase/wc corpus test driver — exercise wc's core counting + UTF-8 logic.
 *
 * Single-file compilation: include all sbase-wc .c files directly.
 * Tests line/word/char counting via pipes (no argv parsing).
 *
 * Known issue: static 2D array element access (sp[i][j]) crashes
 * under FerrugoCC. Tests will be enabled once that bug is fixed.
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

/* wc core logic (extracted from wc.c) */
struct wc_result {
    unsigned long nc;
    unsigned long nl;
    unsigned long nw;
};

static struct wc_result do_wc(FILE *fp, const char *name, char cmode) {
    struct wc_result res;
    res.nc = 0;
    res.nl = 0;
    res.nw = 0;

    int word = 0;
    int rlen;
    Rune c;

    while ((rlen = efgetrune(&c, fp, name))) {
        res.nc += (cmode == 'c') ? rlen : (c != Runeerror);
        if (c == '\n')
            res.nl++;
        if (!isspacerune(c))
            word = 1;
        else if (word) {
            word = 0;
            res.nw++;
        }
    }
    if (word)
        res.nw++;

    return res;
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

    struct wc_result r = do_wc(fp, "test", 'c');
    fclose(fp);

    if (r.nl != 2) return 2;
    if (r.nw != 5) return 3;
    if (r.nc != 24) return 4;
    return 0;
}

static int test_wc_empty(void) {
    const char *text = "";
    FILE *fp = make_pipe_fp(text, 0);
    if (!fp) return 10;

    struct wc_result r = do_wc(fp, "test", 'c');
    fclose(fp);

    if (r.nl != 0) return 11;
    if (r.nw != 0) return 12;
    if (r.nc != 0) return 13;
    return 0;
}

static int test_wc_single_line(void) {
    const char *text = "hello";
    FILE *fp = make_pipe_fp(text, strlen(text));
    if (!fp) return 20;

    struct wc_result r = do_wc(fp, "test", 'c');
    fclose(fp);

    if (r.nl != 0) return 21;
    if (r.nw != 1) return 22;
    if (r.nc != 5) return 23;
    return 0;
}

static int test_wc_utf8(void) {
    /* "café\n" in UTF-8: c a f 0xc3 0xa9 \n = 6 bytes, 5 runes */
    const char text[7] = { 'c', 'a', 'f', '\xc3', '\xa9', '\n', '\0' };
    FILE *fp = make_pipe_fp(text, 6);
    if (!fp) return 30;

    struct wc_result r = do_wc(fp, "test", 'c');
    fclose(fp);
    if (r.nl != 1) return 31;
    if (r.nw != 1) return 32;
    if (r.nc != 6) return 33;

    fp = make_pipe_fp(text, 6);
    if (!fp) return 34;
    r = do_wc(fp, "test", 'm');
    fclose(fp);
    if (r.nl != 1) return 35;
    if (r.nw != 1) return 36;
    if (r.nc != 5) return 37;
    return 0;
}

static int test_wc_spaces(void) {
    const char *text = "  hello   world  \n";
    FILE *fp = make_pipe_fp(text, strlen(text));
    if (!fp) return 40;

    struct wc_result r = do_wc(fp, "test", 'c');
    fclose(fp);

    if (r.nl != 1) return 41;
    if (r.nw != 2) return 42;
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
