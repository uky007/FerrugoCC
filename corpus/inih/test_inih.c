/* inih corpus test driver — parse a small INI string and verify results.
 *
 * Avoids macOS ctype.h inline functions (_DefaultRuneLocale needs GOT).
 * Provides local fgets wrapper to avoid taking address of external fgets
 * (which needs GOT on macOS).
 */

/* Simple isspace to bypass ctype.h */
int isspace(int c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' ||
           c == '\f' || c == '\v';
}

/* Disable assert and ctype.h */
#define NDEBUG
#define _CTYPE_H_
#define assert(x) ((void)0)

#include <string.h>
#include <stdio.h>

/* Wrap fgets so its address can be taken locally */
static char* local_fgets(char* s, int n, FILE* stream) {
    return fgets(s, n, stream);
}
#define fgets local_fgets

/* Include implementation directly (single-file compilation). */
#include "ini.c"

#undef fgets

static int pair_count;
static char last_name[64];
static char last_value[64];

static int handler(void* user, const char* section,
                   const char* name, const char* value)
{
    if (name == 0)
        return 1;
    pair_count++;
    int i;
    for (i = 0; name[i] && i < 63; i++)
        last_name[i] = name[i];
    last_name[i] = '\0';
    for (i = 0; value[i] && i < 63; i++)
        last_value[i] = value[i];
    last_value[i] = '\0';
    return 1;
}

int main(void)
{
    const char* ini_text = "[owner]\nname=John\nage=30\n";

    int result = ini_parse_string(ini_text, handler, 0);
    if (result != 0)
        return 1;
    if (pair_count != 2)
        return 2;
    if (strcmp(last_name, "age") != 0)
        return 3;
    if (strcmp(last_value, "30") != 0)
        return 4;
    return 42;
}
