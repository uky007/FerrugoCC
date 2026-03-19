/* sbase/cat corpus test driver — exercise cat's core I/O logic.
 *
 * Single-file compilation: include all sbase-cat .c files directly.
 * Tests writeall, concat via pipes (no file I/O to avoid sandbox issues).
 */

#define _FORTIFY_SOURCE 0
#define _DONT_USE_CTYPE_INLINE_

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "eprintf.c"
#include "writeall.c"
#include "concat.c"

static int test_writeall_pipe(void) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return 1;

    const char *msg = "hello writeall";
    int len = strlen(msg);
    if (writeall(pipefd[1], msg, len) != len) return 2;
    close(pipefd[1]);

    char buf[64];
    int n = read(pipefd[0], buf, sizeof(buf) - 1);
    close(pipefd[0]);
    buf[n] = '\0';

    if (n != len) return 3;
    if (strcmp(buf, msg) != 0) return 4;
    return 0;
}

static int test_concat_pipe(void) {
    /* Simulate cat: pipe in → concat → pipe out */
    int in_pipe[2];
    int out_pipe[2];
    if (pipe(in_pipe) < 0) return 10;
    if (pipe(out_pipe) < 0) return 11;

    const char *msg = "line one\nline two\n";
    int msg_len = strlen(msg);
    if (writeall(in_pipe[1], msg, msg_len) != msg_len) return 12;
    close(in_pipe[1]);

    int r = concat(in_pipe[0], "<stdin>", out_pipe[1], "<stdout>");
    close(in_pipe[0]);
    close(out_pipe[1]);

    if (r != 0) { close(out_pipe[0]); return 13; }

    char buf[256];
    int n = read(out_pipe[0], buf, sizeof(buf) - 1);
    close(out_pipe[0]);
    buf[n] = '\0';

    if (n != msg_len) return 14;
    if (strcmp(buf, msg) != 0) return 15;
    return 0;
}

static int test_concat_multi(void) {
    /* Concat two separate inputs into same output (like cat file1 file2) */
    int out_pipe[2];
    if (pipe(out_pipe) < 0) return 30;

    /* First input */
    int p1[2];
    if (pipe(p1) < 0) return 31;
    writeall(p1[1], "AAA", 3);
    close(p1[1]);
    if (concat(p1[0], "p1", out_pipe[1], "out") != 0) return 32;
    close(p1[0]);

    /* Second input */
    int p2[2];
    if (pipe(p2) < 0) return 33;
    writeall(p2[1], "BBB", 3);
    close(p2[1]);
    if (concat(p2[0], "p2", out_pipe[1], "out") != 0) return 34;
    close(p2[0]);

    close(out_pipe[1]);

    char buf[64];
    int n = read(out_pipe[0], buf, sizeof(buf) - 1);
    close(out_pipe[0]);
    buf[n] = '\0';

    if (n != 6) return 35;
    if (strcmp(buf, "AAABBB") != 0) return 36;
    return 0;
}

int main(void) {
    int r;

    r = test_writeall_pipe();
    if (r != 0) return r;

    r = test_concat_pipe();
    if (r != 0) return r;

    r = test_concat_multi();
    if (r != 0) return r;

    return 42;
}
