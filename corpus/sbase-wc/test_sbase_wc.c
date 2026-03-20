/* sbase/wc corpus test driver — exercise wc's core counting + UTF-8 logic.
 *
 * Single-file compilation: include all sbase-wc .c files directly.
 * Tests line/word/char counting via pipes (no argv parsing).
 * Struct return by value (≤ 16 bytes) exercises System V ABI.
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

/* Inline eprintf/weprintf */
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

/* wc core logic — struct return exercises ABI (≤ 16 bytes → RAX+RDX) */
struct wc_result {
    unsigned long nl;
    unsigned long nw;
};

static struct wc_result do_wc(FILE *fp, const char *name) {
    struct wc_result res;
    res.nl = 0;
    res.nw = 0;

    int word = 0;
    int rlen;
    Rune c;

    while ((rlen = efgetrune(&c, fp, name))) {
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
    struct wc_result r = do_wc(fp, "test");
    fclose(fp);
    if (r.nl != 2) return 2;
    if (r.nw != 5) return 3;
    return 0;
}

static int test_wc_empty(void) {
    FILE *fp = make_pipe_fp("", 0);
    if (!fp) return 10;
    struct wc_result r = do_wc(fp, "test");
    fclose(fp);
    if (r.nl != 0) return 11;
    if (r.nw != 0) return 12;
    return 0;
}

static int test_wc_single_line(void) {
    FILE *fp = make_pipe_fp("hello", 5);
    if (!fp) return 20;
    struct wc_result r = do_wc(fp, "test");
    fclose(fp);
    if (r.nl != 0) return 21;
    if (r.nw != 1) return 22;
    return 0;
}

static int test_wc_utf8(void) {
    const char text[7] = { 'c', 'a', 'f', '\xc3', '\xa9', '\n', '\0' };
    FILE *fp = make_pipe_fp(text, 6);
    if (!fp) return 30;
    struct wc_result r = do_wc(fp, "test");
    fclose(fp);
    if (r.nl != 1) return 31;
    if (r.nw != 1) return 32;
    return 0;
}

static int test_wc_spaces(void) {
    const char *text = "  hello   world  \n";
    FILE *fp = make_pipe_fp(text, strlen(text));
    if (!fp) return 40;
    struct wc_result r = do_wc(fp, "test");
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
