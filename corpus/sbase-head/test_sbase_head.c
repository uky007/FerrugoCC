/* sbase/head corpus test driver — exercise head's line-counting logic.
 *
 * Tests head() directly via pipes (no argv/terminal I/O).
 * Exercises: getline, fwrite, ferror, line counting, multiple inputs.
 */

#define _FORTIFY_SOURCE 0
#define _DONT_USE_CTYPE_INLINE_

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <errno.h>

#include "head_util.h"

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

int fshut(FILE *fp, const char *fname) {
    fflush(fp);
    if (ferror(fp)) return 1;
    return 0;
}

/* strtonum (simplified — enough for head) */
long long estrtonum(const char *s, long long min, long long max) {
    char *end;
    long long val = strtoll(s, &end, 10);
    if (*end != '\0' || val < min || val > max)
        eprintf("strtonum %s: invalid\n", s);
    return val;
}

/* head() from sbase — included directly */
static void
head(FILE *fp, const char *fname, unsigned long n)
{
    char *buf = NULL;
    unsigned long size = 0;
    unsigned long i = 0;
    long len;

    while (i < n && (len = getline(&buf, &size, fp)) > 0) {
        fwrite(buf, 1, len, stdout);
        i += (len && (buf[len - 1] == '\n'));
    }
    free(buf);
}

/* Test helpers */
static FILE *pipe_input(const char *data) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return NULL;
    write(pipefd[1], data, strlen(data));
    close(pipefd[1]);
    return fdopen(pipefd[0], "r");
}

static int capture_head(const char *input, unsigned long n,
                        char *out, int outsz) {
    FILE *in_fp = pipe_input(input);
    if (!in_fp) return -1;

    /* Redirect stdout to a pipe to capture output */
    int out_pipe[2];
    if (pipe(out_pipe) < 0) { fclose(in_fp); return -1; }

    int saved_stdout = dup(1);
    dup2(out_pipe[1], 1);
    close(out_pipe[1]);

    head(in_fp, "test", n);
    fflush(stdout);

    dup2(saved_stdout, 1);
    close(saved_stdout);
    fclose(in_fp);

    int nread = read(out_pipe[0], out, outsz - 1);
    close(out_pipe[0]);
    if (nread < 0) nread = 0;
    out[nread] = '\0';
    return nread;
}

/* Tests */
static int test_head_basic(void) {
    char out[256];
    const char *input = "line1\nline2\nline3\nline4\nline5\n";
    int n = capture_head(input, 3, out, sizeof(out));
    if (n < 0) return 1;
    if (strcmp(out, "line1\nline2\nline3\n") != 0) return 2;
    return 0;
}

static int test_head_all(void) {
    char out[256];
    const char *input = "one\ntwo\n";
    int n = capture_head(input, 10, out, sizeof(out));
    if (n < 0) return 10;
    if (strcmp(out, "one\ntwo\n") != 0) return 11;
    return 0;
}

static int test_head_one(void) {
    char out[256];
    const char *input = "first\nsecond\nthird\n";
    int n = capture_head(input, 1, out, sizeof(out));
    if (n < 0) return 20;
    if (strcmp(out, "first\n") != 0) return 21;
    return 0;
}

static int test_head_empty(void) {
    char out[256];
    int n = capture_head("", 5, out, sizeof(out));
    if (n < 0) return 30;
    if (out[0] != '\0') return 31;
    return 0;
}

static int test_head_no_trailing_newline(void) {
    char out[256];
    const char *input = "line1\nline2\nno-newline";
    int n = capture_head(input, 3, out, sizeof(out));
    if (n < 0) return 40;
    /* head reads 3 lines: "line1\n" (line 1), "line2\n" (line 2),
       "no-newline" (not a complete line, doesn't count) → 3 lines or less */
    if (strcmp(out, "line1\nline2\nno-newline") != 0) return 41;
    return 0;
}

int main(void) {
    int r;
    r = test_head_basic();
    if (r != 0) return r;
    r = test_head_all();
    if (r != 0) return r;
    r = test_head_one();
    if (r != 0) return r;
    r = test_head_empty();
    if (r != 0) return r;
    r = test_head_no_trailing_newline();
    if (r != 0) return r;
    return 42;
}
