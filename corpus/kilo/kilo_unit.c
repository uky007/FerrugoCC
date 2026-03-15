/* kilo_unit.c — kilo の主要 C パターンを抽出したユニットテスト。
 * ターミナル I/O なしで kilo のコード生成品質を検証する。
 *
 * テスト対象パターン:
 *   - struct + char 配列メンバ + ポインタメンバ
 *   - realloc / memcpy / memmove / memset
 *   - snprintf 文字列フォーマット
 *   - switch/case + enum 定数
 *   - 文字分類 (is_separator 相当)
 *   - struct ポインタ経由のメンバ操作
 *   - ビット演算 (&= ~(...))
 *   - 関数ポインタ typedef + コールバック
 *   - タブ展開 (editorUpdateRow 相当)
 *   - 複数行管理 (InsertRow / DelRow)
 *   - 行連結 (editorRowAppendString 相当)
 *   - 検索 (strstr + ポインタ演算)
 *   - va_list 経由の vsnprintf (editorSetStatusMessage 相当)
 *   - ファイル読み込み (editorOpen 相当)
 */

#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <stdarg.h>

/* ── 1. struct + char 配列メンバ ── */

struct editorRow {
    int size;
    int rsize;
    char *chars;
    char *render;
};

struct abuf {
    char *b;
    int len;
};

static void abAppend(struct abuf *ab, const char *s, int len) {
    char *new_buf = realloc(ab->b, ab->len + len);
    if (new_buf == NULL) return;
    memcpy(new_buf + ab->len, s, len);
    ab->b = new_buf;
    ab->len += len;
}

static void abFree(struct abuf *ab) {
    free(ab->b);
}

static int test_abuf(void) {
    struct abuf ab;
    ab.b = NULL;
    ab.len = 0;

    abAppend(&ab, "Hello", 5);
    abAppend(&ab, " ", 1);
    abAppend(&ab, "World", 5);

    if (ab.len != 11) return 1;
    if (memcmp(ab.b, "Hello World", 11) != 0) return 2;

    abFree(&ab);
    return 0;
}

/* ── 2. switch/case + enum 定数 ── */

#define HL_NORMAL 0
#define HL_COMMENT 2
#define HL_KEYWORD1 4
#define HL_KEYWORD2 5
#define HL_STRING 6
#define HL_NUMBER 7
#define HL_MATCH 8

static int editorSyntaxToColor(int hl) {
    switch(hl) {
    case HL_COMMENT:
    case 3: /* HL_MLCOMMENT */
        return 36;
    case HL_KEYWORD1: return 33;
    case HL_KEYWORD2: return 32;
    case HL_STRING: return 35;
    case HL_NUMBER: return 31;
    case HL_MATCH: return 34;
    default: return 37;
    }
}

static int test_syntax_color(void) {
    if (editorSyntaxToColor(HL_COMMENT) != 36) return 10;
    if (editorSyntaxToColor(HL_KEYWORD1) != 33) return 11;
    if (editorSyntaxToColor(HL_STRING) != 35) return 12;
    if (editorSyntaxToColor(HL_NUMBER) != 31) return 13;
    if (editorSyntaxToColor(HL_NORMAL) != 37) return 14;
    if (editorSyntaxToColor(HL_MATCH) != 34) return 15;
    return 0;
}

/* ── 3. is_separator — 文字分類 ── */

static int is_separator(int c) {
    return c == '\0' || c == ' ' || c == '\t' || c == '\n' ||
           c == '\r' || c == ',' || c == '.' || c == '(' ||
           c == ')' || c == '+' || c == '-' || c == '/' ||
           c == '*' || c == '=' || c == '~' || c == '%' ||
           c == '<' || c == '>' || c == '[' || c == ']' ||
           c == '{' || c == '}' || c == ';' || c == ':' ||
           c == '"' || c == '\'';
}

static int test_is_separator(void) {
    if (!is_separator(' ')) return 20;
    if (!is_separator(';')) return 21;
    if (!is_separator('(')) return 22;
    if (is_separator('a')) return 23;
    if (is_separator('0')) return 24;
    if (is_separator('_')) return 25;
    if (!is_separator('\0')) return 26;
    return 0;
}

/* ── 4. struct ポインタ操作 + realloc + memmove ── */

static void editorRowInsertChar(struct editorRow *row, int at, int c) {
    if (at > row->size) {
        int padlen = at - row->size;
        row->chars = realloc(row->chars, row->size + padlen + 2);
        memset(row->chars + row->size, ' ', padlen);
        row->chars[at] = c;
        row->chars[at + 1] = '\0';
        row->size = at + 1;
    } else {
        row->chars = realloc(row->chars, row->size + 2);
        memmove(row->chars + at + 1, row->chars + at, row->size - at + 1);
        row->chars[at] = c;
        row->size++;
    }
}

static void editorRowDelChar(struct editorRow *row, int at) {
    if (at >= row->size) return;
    memmove(row->chars + at, row->chars + at + 1, row->size - at);
    row->size--;
}

static int test_row_edit(void) {
    struct editorRow row;
    row.size = 5;
    row.chars = malloc(16);
    memcpy(row.chars, "Hello", 6);

    /* Insert 'X' at position 2: "HeXllo" */
    editorRowInsertChar(&row, 2, 'X');
    if (row.size != 6) return 30;
    if (row.chars[2] != 'X') return 31;
    if (row.chars[3] != 'l') return 32;

    /* Delete char at position 2: "Hello" again (but "Hllo" wait...) */
    /* Actually "HeXllo" -> delete at 2 -> "Hello" */
    editorRowDelChar(&row, 2);
    if (row.size != 5) return 33;
    if (memcmp(row.chars, "Hello", 5) != 0) return 34;

    free(row.chars);
    return 0;
}

/* ── 5. snprintf フォーマット ── */

static int test_snprintf(void) {
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "Line %d/%d", 10, 42);
    if (n <= 0) return 40;
    if (strcmp(buf, "Line 10/42") != 0) return 41;

    n = snprintf(buf, sizeof(buf), "%d cols", 80);
    if (strcmp(buf, "80 cols") != 0) return 42;

    return 0;
}

/* ── 6. ビット演算 — termios パターン ── */

static int test_bitwise_mask(void) {
    unsigned int flags = 0xFF;

    /* kilo パターン: flags &= ~(BIT1 | BIT2 | BIT3) */
    flags &= ~(0x01 | 0x04 | 0x10);
    /* 0xFF & ~0x15 = 0xFF & 0xEA = 0xEA */
    if (flags != 0xEA) return 50;

    /* flags |= BIT */
    flags |= 0x01;
    if (flags != 0xEB) return 51;

    return 0;
}

/* ── 7. typedef 関数ポインタ + コールバック ── */

typedef int (*transform_t)(int);

static int double_it(int x) { return x * 2; }
static int inc_it(int x) { return x + 1; }

static int apply_transform(transform_t fn, int val) {
    return fn(val);
}

static int test_fn_ptr_callback(void) {
    if (apply_transform(double_it, 21) != 42) return 60;
    if (apply_transform(inc_it, 41) != 42) return 61;

    /* 配列パターン */
    transform_t ops[2];
    ops[0] = double_it;
    ops[1] = inc_it;
    int r = ops[0](20) + ops[1](1);
    if (r != 42) return 62;

    return 0;
}

/* ── 8. editorRowsToString 相当 — 複数行結合 ── */

static char *rowsToString(char **rows, int numrows, int *buflen) {
    int totlen = 0;
    int j;
    for (j = 0; j < numrows; j++) {
        totlen += strlen(rows[j]) + 1; /* +1 for newline */
    }
    *buflen = totlen;

    char *buf = malloc(totlen + 1);
    char *p = buf;
    for (j = 0; j < numrows; j++) {
        int len = strlen(rows[j]);
        memcpy(p, rows[j], len);
        p += len;
        *p = '\n';
        p++;
    }
    *p = '\0';
    return buf;
}

static int test_rows_to_string(void) {
    char *rows[3];
    rows[0] = "Hello";
    rows[1] = "World";
    rows[2] = "!";

    int buflen;
    char *result = rowsToString(rows, 3, &buflen);
    /* "Hello\n" + "World\n" + "!\n" = 6+6+2 = 14 */
    if (buflen != 14) { free(result); return 70; }
    if (strcmp(result, "Hello\nWorld\n!\n") != 0) { free(result); return 71; }
    free(result);
    return 0;
}

/* ── 9. タブ展開 — editorUpdateRow 相当 ── */

#define KILO_TAB_STOP 8

static char *renderRow(const char *chars, int size, int *rsize) {
    int tabs = 0;
    int j;
    for (j = 0; j < size; j++)
        if (chars[j] == '\t') tabs++;

    char *render = malloc(size + tabs * 8 + 1);
    int idx = 0;
    for (j = 0; j < size; j++) {
        if (chars[j] == '\t') {
            render[idx++] = ' ';
            while ((idx + 1) % KILO_TAB_STOP != 0) render[idx++] = ' ';
        } else {
            render[idx++] = chars[j];
        }
    }
    render[idx] = '\0';
    *rsize = idx;
    return render;
}

static int test_tab_expansion(void) {
    int rsize;
    /* "A\tB" → "A       B" (A + 7 spaces + B = 9 chars, tab stop at 8) */
    /* Actually: idx=0 'A', idx=1 tab: render[1]=' ', while (2+1)%8!=0 → spaces at 2..6,
       then idx=7, then 'B' at idx=7. Wait let me trace:
       j=0: chars[0]='A' → render[0]='A', idx=1
       j=1: chars[1]='\t' → render[1]=' ', idx=2; while (2+1)%8!=0 → render[2]=' ' idx=3,
            (3+1)%8!=0 → render[3]=' ' idx=4, (4+1)%8!=0 → render[4]=' ' idx=5,
            (5+1)%8!=0 → render[5]=' ' idx=6, (6+1)%8!=0 → render[6]=' ' idx=7,
            (7+1)%8==0 → stop
       j=2: chars[2]='B' → render[7]='B', idx=8
       rsize = 8 */
    char *r = renderRow("A\tB", 3, &rsize);
    if (rsize != 8) { free(r); return 80; }
    if (r[0] != 'A') { free(r); return 81; }
    if (r[7] != 'B') { free(r); return 82; }
    /* all middle chars should be spaces */
    int i;
    for (i = 1; i < 7; i++) {
        if (r[i] != ' ') { free(r); return 83; }
    }
    free(r);

    /* no tabs → same length */
    r = renderRow("Hello", 5, &rsize);
    if (rsize != 5) { free(r); return 84; }
    if (strcmp(r, "Hello") != 0) { free(r); return 85; }
    free(r);

    return 0;
}

/* ── 10. 複数行管理 — InsertRow + DelRow + RowsToString ── */

struct editorState {
    int numrows;
    struct editorRow *row;
};

static void stateInsertRow(struct editorState *s, int at, const char *text, int len) {
    if (at > s->numrows) return;
    s->row = realloc(s->row, sizeof(struct editorRow) * (s->numrows + 1));
    if (at != s->numrows) {
        memmove(s->row + at + 1, s->row + at,
                sizeof(struct editorRow) * (s->numrows - at));
    }
    s->row[at].size = len;
    s->row[at].chars = malloc(len + 1);
    memcpy(s->row[at].chars, text, len);
    s->row[at].chars[len] = '\0';
    s->row[at].render = NULL;
    s->row[at].rsize = 0;
    s->numrows++;
}

static void stateDelRow(struct editorState *s, int at) {
    if (at >= s->numrows) return;
    free(s->row[at].chars);
    free(s->row[at].render);
    if (at < s->numrows - 1) {
        memmove(s->row + at, s->row + at + 1,
                sizeof(struct editorRow) * (s->numrows - at - 1));
    }
    s->numrows--;
}

static void stateFreeAll(struct editorState *s) {
    int i;
    for (i = 0; i < s->numrows; i++) {
        free(s->row[i].chars);
        free(s->row[i].render);
    }
    free(s->row);
}

static int test_multi_row(void) {
    struct editorState s;
    s.numrows = 0;
    s.row = NULL;

    stateInsertRow(&s, 0, "First", 5);
    stateInsertRow(&s, 1, "Second", 6);
    stateInsertRow(&s, 2, "Third", 5);

    if (s.numrows != 3) return 90;
    if (strcmp(s.row[0].chars, "First") != 0) return 91;
    if (strcmp(s.row[1].chars, "Second") != 0) return 92;
    if (strcmp(s.row[2].chars, "Third") != 0) return 93;

    /* Insert in the middle */
    stateInsertRow(&s, 1, "Middle", 6);
    if (s.numrows != 4) return 94;
    if (strcmp(s.row[1].chars, "Middle") != 0) return 95;
    if (strcmp(s.row[2].chars, "Second") != 0) return 96;

    /* Delete from middle */
    stateDelRow(&s, 1);
    if (s.numrows != 3) return 97;
    if (strcmp(s.row[0].chars, "First") != 0) return 98;
    if (strcmp(s.row[1].chars, "Second") != 0) return 99;

    stateFreeAll(&s);
    return 0;
}

/* ── 11. 行連結 — editorRowAppendString 相当 ── */

static void rowAppendString(struct editorRow *row, const char *s, int len) {
    row->chars = realloc(row->chars, row->size + len + 1);
    memcpy(row->chars + row->size, s, len);
    row->size += len;
    row->chars[row->size] = '\0';
}

static int test_row_append(void) {
    struct editorRow row;
    row.size = 5;
    row.chars = malloc(16);
    memcpy(row.chars, "Hello", 6);

    rowAppendString(&row, " World", 6);
    if (row.size != 11) return 100;
    if (strcmp(row.chars, "Hello World") != 0) return 101;

    rowAppendString(&row, "!", 1);
    if (row.size != 12) return 102;
    if (strcmp(row.chars, "Hello World!") != 0) return 103;

    free(row.chars);
    return 0;
}

/* ── 12. 検索 — strstr ベースの行マッチング ── */

static int findInRows(char **rows, int numrows, const char *query) {
    int i;
    for (i = 0; i < numrows; i++) {
        if (strstr(rows[i], query) != NULL) {
            return i;
        }
    }
    return -1;
}

static int test_search(void) {
    char *rows[4];
    rows[0] = "int main(void) {";
    rows[1] = "    int x = 42;";
    rows[2] = "    return x;";
    rows[3] = "}";

    if (findInRows(rows, 4, "main") != 0) return 110;
    if (findInRows(rows, 4, "42") != 1) return 111;
    if (findInRows(rows, 4, "return") != 2) return 112;
    if (findInRows(rows, 4, "}") != 3) return 113;
    if (findInRows(rows, 4, "notfound") != -1) return 114;

    /* pointer arithmetic on match */
    char *match = strstr(rows[1], "42");
    int offset = match - rows[1];
    if (offset != 12) return 115;

    return 0;
}

/* ── 13. ステータスメッセージフォーマット — vsnprintf 相当 ── */

static int test_status_message(void) {
    char statusmsg[80];

    snprintf(statusmsg, sizeof(statusmsg),
             "HELP: Ctrl-S = save | Ctrl-Q = quit | Ctrl-F = find");
    if (strlen(statusmsg) == 0) return 120;
    if (strstr(statusmsg, "Ctrl-Q") == NULL) return 121;

    snprintf(statusmsg, sizeof(statusmsg),
             "Search: %s (Use ESC/Arrows/Enter)", "hello");
    if (strstr(statusmsg, "hello") == NULL) return 122;

    /* WARNING message with %d */
    snprintf(statusmsg, sizeof(statusmsg),
             "WARNING!!! Press Ctrl-Q %d more times to quit.", 3);
    if (strstr(statusmsg, "3") == NULL) return 123;

    /* Truncation: long message into small buffer */
    char small[16];
    snprintf(small, sizeof(small), "This is a very long message");
    if (strlen(small) != 15) return 124;  /* 15 chars + null */

    return 0;
}

/* ── 14. editorSetStatusMessage 相当 — 実 va_list 経路 ── */

static char statusmsg_buf[256];

static void setStatusMessage(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(statusmsg_buf, sizeof(statusmsg_buf), fmt, ap);
    va_end(ap);
}

static int test_va_list_status(void) {
    /* basic string + int */
    setStatusMessage("Line %d/%d", 10, 100);
    if (strcmp(statusmsg_buf, "Line 10/100") != 0) return 130;

    /* string arg */
    setStatusMessage("Search: %s", "hello");
    if (strcmp(statusmsg_buf, "Search: hello") != 0) return 131;

    /* multiple args of different types */
    setStatusMessage("%d bytes written on disk", 4096);
    if (strcmp(statusmsg_buf, "4096 bytes written on disk") != 0) return 132;

    /* empty format */
    setStatusMessage("");
    if (statusmsg_buf[0] != '\0') return 133;

    /* WARNING message */
    setStatusMessage("WARNING!!! Press Ctrl-Q %d more times to quit.", 3);
    if (strstr(statusmsg_buf, "3 more times") == NULL) return 134;

    /* long message with multiple arg types */
    setStatusMessage("File '%s' saved (%d lines, %d bytes)", "test.c", 42, 1024);
    if (strstr(statusmsg_buf, "test.c") == NULL) return 135;
    if (strstr(statusmsg_buf, "42 lines") == NULL) return 136;
    if (strstr(statusmsg_buf, "1024 bytes") == NULL) return 137;

    return 0;
}

/* ── 15. ファイル読み込み — editorOpen 相当の最小パス ── */

struct fileState {
    int numrows;
    char **rows;
};

static int fileOpen(struct fileState *fs, const char *filename) {
    FILE *fp = fopen(filename, "r");
    if (!fp) return -1;

    char line[1024];
    while (fgets(line, sizeof(line), fp) != NULL) {
        int len = strlen(line);
        /* Strip trailing \n and \r (like kilo) */
        while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r'))
            line[--len] = '\0';

        fs->rows = realloc(fs->rows, sizeof(char *) * (fs->numrows + 1));
        fs->rows[fs->numrows] = malloc(len + 1);
        memcpy(fs->rows[fs->numrows], line, len + 1);
        fs->numrows++;
    }
    fclose(fp);
    return 0;
}

static void fileFree(struct fileState *fs) {
    int i;
    for (i = 0; i < fs->numrows; i++)
        free(fs->rows[i]);
    free(fs->rows);
}

static int test_file_read(void) {
    /* Write a temp file, read it back, verify contents */
    const char *tmppath = "/tmp/kilo_unit_test.txt";
    FILE *fp = fopen(tmppath, "w");
    if (!fp) return 140;
    fprintf(fp, "First line\n");
    fprintf(fp, "Second line\n");
    fprintf(fp, "Third\n");
    fclose(fp);

    struct fileState fs;
    fs.numrows = 0;
    fs.rows = NULL;

    if (fileOpen(&fs, tmppath) != 0) return 141;
    if (fs.numrows != 3) return 142;
    if (strcmp(fs.rows[0], "First line") != 0) return 143;
    if (strcmp(fs.rows[1], "Second line") != 0) return 144;
    if (strcmp(fs.rows[2], "Third") != 0) return 145;

    fileFree(&fs);

    /* Non-existent file → error */
    struct fileState fs2;
    fs2.numrows = 0;
    fs2.rows = NULL;
    if (fileOpen(&fs2, "/tmp/kilo_unit_NONEXISTENT_12345.txt") != -1) return 146;

    /* Empty file */
    fp = fopen(tmppath, "w");
    if (!fp) return 147;
    fclose(fp);

    struct fileState fs3;
    fs3.numrows = 0;
    fs3.rows = NULL;
    if (fileOpen(&fs3, tmppath) != 0) return 148;
    if (fs3.numrows != 0) return 149;
    fileFree(&fs3);

    /* Cleanup */
    remove(tmppath);
    return 0;
}

/* ── メインテストランナー ── */

int main(void) {
    int r;

    r = test_abuf();
    if (r != 0) return r;

    r = test_syntax_color();
    if (r != 0) return r;

    r = test_is_separator();
    if (r != 0) return r;

    r = test_row_edit();
    if (r != 0) return r;

    r = test_snprintf();
    if (r != 0) return r;

    r = test_bitwise_mask();
    if (r != 0) return r;

    r = test_fn_ptr_callback();
    if (r != 0) return r;

    r = test_rows_to_string();
    if (r != 0) return r;

    r = test_tab_expansion();
    if (r != 0) return r;

    r = test_multi_row();
    if (r != 0) return r;

    r = test_row_append();
    if (r != 0) return r;

    r = test_search();
    if (r != 0) return r;

    r = test_status_message();
    if (r != 0) return r;

    r = test_va_list_status();
    if (r != 0) return r;

    r = test_file_read();
    if (r != 0) return r;

    /* 全テスト通過 */
    return 42;
}
