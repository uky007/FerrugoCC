/* sbase/cut corpus test driver — exercise field/byte cutting logic.
 *
 * Single-file compilation: include cut.c directly (with main renamed).
 * Tests parselist, seek, cut via pipes.
 */

#define _FORTIFY_SOURCE 0
#define _DONT_USE_CTYPE_INLINE_

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <errno.h>

#include "cut_util.h"

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

void *ereallocarray(void *p, size_t n, size_t s) {
    void *r = realloc(p, n * s);
    if (!r) eprintf("realloc:");
    return r;
}

/* Minimal fullrune/charntorune for byte mode (ASCII only for tests) */
int runelen(int r) { return (r <= 0x7f) ? 1 : 0; }
int charntorune(int *p, const char *s, size_t len) {
    if (len == 0) return 0;
    *p = (unsigned char)s[0];
    return 1;
}
int fullrune(const char *s, size_t len) {
    int r;
    return charntorune(&r, s, len) > 0;
}

/* Minimal unescape (just pass through for tests) */
size_t unescape(char *s) { return strlen(s); }

#include "memmem.c"

/* Include cut.c but skip its main (which uses stdin directly) */
#define main cut_main_unused
#define usage cut_usage_unused
#include "cut.c"
#undef main
#undef usage

/* ── Test helpers ── */

static int capture_cut(const char *input, char mode_ch, const char *list_str,
                       const char *delim_str, char *out, int outsz) {
    /* Reset globals */
    list = NULL;
    mode = mode_ch;
    nflag = 0;
    sflag = 0;
    if (delim_str) {
        delim = (char *)delim_str;
        delimlen = strlen(delim_str);
    } else {
        delim = "\t";
        delimlen = 1;
    }

    /* Parse the field/byte list */
    char list_buf[64];
    strncpy(list_buf, list_str, sizeof(list_buf) - 1);
    list_buf[sizeof(list_buf) - 1] = '\0';
    parselist(list_buf);

    /* Create input pipe */
    int in_pipe[2];
    if (pipe(in_pipe) < 0) return -1;
    write(in_pipe[1], input, strlen(input));
    close(in_pipe[1]);
    FILE *fp = fdopen(in_pipe[0], "r");

    /* Redirect stdout to capture */
    int out_pipe[2];
    if (pipe(out_pipe) < 0) { fclose(fp); return -1; }
    int saved = dup(1);
    dup2(out_pipe[1], 1);
    close(out_pipe[1]);

    cut(fp, "test");
    fflush(stdout);

    dup2(saved, 1);
    close(saved);
    fclose(fp);

    int n = read(out_pipe[0], out, outsz - 1);
    close(out_pipe[0]);
    if (n < 0) n = 0;
    out[n] = '\0';
    return n;
}

/* ── Tests ── */

static int test_cut_fields(void) {
    char out[256];
    /* Tab-delimited: "a\tb\tc\n" → field 2 = "b" */
    capture_cut("a\tb\tc\n", 'f', "2", NULL, out, sizeof(out));
    if (strcmp(out, "b\n") != 0) return 1;

    /* Multiple fields: fields 1,3 */
    capture_cut("a\tb\tc\n", 'f', "1,3", NULL, out, sizeof(out));
    if (strcmp(out, "a\tc\n") != 0) return 2;

    return 0;
}

static int test_cut_bytes(void) {
    char out[256];
    /* Bytes 1-3 of "hello\n" = "hel" */
    capture_cut("hello\n", 'b', "1-3", NULL, out, sizeof(out));
    if (strcmp(out, "hel\n") != 0) return 10;

    /* Single byte */
    capture_cut("abcdef\n", 'b', "4", NULL, out, sizeof(out));
    if (strcmp(out, "d\n") != 0) return 11;

    return 0;
}

static int test_cut_custom_delim(void) {
    char out[256];
    /* Colon-delimited */
    capture_cut("root:x:0:0\n", 'f', "1", ":", out, sizeof(out));
    if (strcmp(out, "root\n") != 0) return 20;

    capture_cut("root:x:0:0\n", 'f', "3", ":", out, sizeof(out));
    if (strcmp(out, "0\n") != 0) return 21;

    return 0;
}

static int test_cut_multiline(void) {
    char out[256];
    capture_cut("a\tb\nc\td\n", 'f', "2", NULL, out, sizeof(out));
    if (strcmp(out, "b\nd\n") != 0) return 30;

    return 0;
}

int main(void) {
    int r;
    r = test_cut_fields();
    if (r != 0) return r;
    r = test_cut_bytes();
    if (r != 0) return r;
    r = test_cut_custom_delim();
    if (r != 0) return r;
    r = test_cut_multiline();
    if (r != 0) return r;
    return 42;
}
