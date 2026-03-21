/* sbase/uniq corpus test driver — exercise line dedup logic.
 *
 * Tests uniq(), uniqline(), uniqfinish() via pipes.
 * Exercises: memcmp, getline, realloc, consecutive line comparison.
 */

#define _FORTIFY_SOURCE 0
#define _DONT_USE_CTYPE_INLINE_

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <ctype.h>

#include "uniq_util.h"

/* Inline utilities */
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

void *erealloc(void *p, size_t n) {
    void *r = realloc(p, n);
    if (!r) eprintf("realloc:");
    return r;
}

int fshut(FILE *fp, const char *f) {
    fflush(fp);
    return ferror(fp) ? 1 : 0;
}

long long estrtonum(const char *s, long long min, long long max) {
    char *end;
    long long val = strtoll(s, &end, 10);
    if (*end != '\0' || val < min || val > max)
        eprintf("strtonum %s: invalid\n", s);
    return val;
}

/* Include uniq.c (rename main) */
#define main uniq_main_unused
#define usage uniq_usage_unused
#include "uniq.c"
#undef main
#undef usage

/* Reset globals between tests */
static void reset_uniq(void) {
    countfmt = "";
    dflag = 0;
    uflag = 0;
    fskip = 0;
    sskip = 0;
    if (prevl.data) { free(prevl.data); prevl.data = 0; }
    prevl.len = 0;
    prevoff = -1;
    prevlinecount = 0;
}

/* Test helper: pipe input through uniq, capture output */
static int run_uniq(const char *input, char *out, int outsz) {
    int in_pipe[2];
    if (pipe(in_pipe) < 0) return -1;
    write(in_pipe[1], input, strlen(input));
    close(in_pipe[1]);
    FILE *ifp = fdopen(in_pipe[0], "r");

    int out_pipe[2];
    if (pipe(out_pipe) < 0) { fclose(ifp); return -1; }
    int saved = dup(1);
    dup2(out_pipe[1], 1);
    close(out_pipe[1]);

    uniq(ifp, stdout);
    uniqfinish(stdout);
    fflush(stdout);

    dup2(saved, 1);
    close(saved);
    fclose(ifp);

    int n = read(out_pipe[0], out, outsz - 1);
    close(out_pipe[0]);
    if (n < 0) n = 0;
    out[n] = '\0';
    return n;
}

/* ── Tests ── */

static int test_basic_dedup(void) {
    reset_uniq();
    char out[256];
    run_uniq("aaa\naaa\nbbb\nccc\nccc\n", out, sizeof(out));
    if (strcmp(out, "aaa\nbbb\nccc\n") != 0) return 1;
    return 0;
}

static int test_no_duplicates(void) {
    reset_uniq();
    char out[256];
    run_uniq("a\nb\nc\n", out, sizeof(out));
    if (strcmp(out, "a\nb\nc\n") != 0) return 10;
    return 0;
}

static int test_all_same(void) {
    reset_uniq();
    char out[256];
    run_uniq("x\nx\nx\nx\n", out, sizeof(out));
    if (strcmp(out, "x\n") != 0) return 20;
    return 0;
}

static int test_empty(void) {
    reset_uniq();
    char out[256];
    run_uniq("", out, sizeof(out));
    if (out[0] != '\0') return 30;
    return 0;
}

static int test_single_line(void) {
    reset_uniq();
    char out[256];
    run_uniq("hello\n", out, sizeof(out));
    if (strcmp(out, "hello\n") != 0) return 40;
    return 0;
}

int main(void) {
    int r;
    r = test_basic_dedup();
    if (r != 0) return r;
    r = test_no_duplicates();
    if (r != 0) return r;
    r = test_all_same();
    if (r != 0) return r;
    r = test_empty();
    if (r != 0) return r;
    r = test_single_line();
    if (r != 0) return r;
    return 42;
}
