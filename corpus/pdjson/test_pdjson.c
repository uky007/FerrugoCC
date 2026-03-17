/* pdjson corpus test driver — exercise core JSON parsing API.
 *
 * Uses json_open_string (buffer-based) to avoid FILE * complications.
 * Single-file compilation: include pdjson.c directly.
 */

/* Suppress macOS ctype inline issues */
#define _DONT_USE_CTYPE_INLINE_
#define _FORTIFY_SOURCE 0

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "pdjson.h"
#include "pdjson.c"

int main(void)
{
    /* Test 1: Parse simple object with string and number */
    {
        struct json_stream s;
        json_open_string(&s, "{\"name\": \"pdjson\", \"version\": 1}");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_next(&s);
        if (t != JSON_OBJECT) return 1;

        t = json_next(&s);
        if (t != JSON_STRING) return 2;
        if (strcmp(json_get_string(&s, 0), "name") != 0) return 3;

        t = json_next(&s);
        if (t != JSON_STRING) return 4;
        if (strcmp(json_get_string(&s, 0), "pdjson") != 0) return 5;

        t = json_next(&s);
        if (t != JSON_STRING) return 6;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 7;
        if (json_get_number(&s) != 1.0) return 8;

        t = json_next(&s);
        if (t != JSON_OBJECT_END) return 9;

        t = json_next(&s);
        if (t != JSON_DONE) return 10;

        json_close(&s);
    }

    /* Test 2: Parse array with mixed types */
    {
        struct json_stream s;
        json_open_string(&s, "[1, \"two\", true, false, null]");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_next(&s);
        if (t != JSON_ARRAY) return 11;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 12;

        t = json_next(&s);
        if (t != JSON_STRING) return 13;
        if (strcmp(json_get_string(&s, 0), "two") != 0) return 14;

        t = json_next(&s);
        if (t != JSON_TRUE) return 15;

        t = json_next(&s);
        if (t != JSON_FALSE) return 16;

        t = json_next(&s);
        if (t != JSON_NULL) return 17;

        t = json_next(&s);
        if (t != JSON_ARRAY_END) return 18;

        t = json_next(&s);
        if (t != JSON_DONE) return 19;

        json_close(&s);
    }

    /* Test 3: Nested objects and json_skip */
    {
        struct json_stream s;
        json_open_string(&s, "{\"a\": [1, 2], \"b\": {\"c\": 3}}");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_next(&s);
        if (t != JSON_OBJECT) return 20;

        t = json_next(&s);
        if (t != JSON_STRING) return 21;

        t = json_skip(&s);
        if (t != JSON_ARRAY) return 22;

        t = json_next(&s);
        if (t != JSON_STRING) return 23;

        t = json_next(&s);
        if (t != JSON_OBJECT) return 24;

        t = json_next(&s);
        if (t != JSON_STRING) return 25;
        if (strcmp(json_get_string(&s, 0), "c") != 0) return 26;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 27;
        if (json_get_number(&s) != 3.0) return 28;

        t = json_next(&s);
        if (t != JSON_OBJECT_END) return 29;

        t = json_next(&s);
        if (t != JSON_OBJECT_END) return 30;

        t = json_next(&s);
        if (t != JSON_DONE) return 31;

        json_close(&s);
    }

    /* Test 4: json_peek */
    {
        struct json_stream s;
        json_open_string(&s, "[42]");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_peek(&s);
        if (t != JSON_ARRAY) return 32;

        t = json_peek(&s);
        if (t != JSON_ARRAY) return 33;

        t = json_next(&s);
        if (t != JSON_ARRAY) return 34;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 35;
        if (json_get_number(&s) != 42.0) return 36;

        t = json_next(&s);
        if (t != JSON_ARRAY_END) return 37;

        json_close(&s);
    }

    /* Test 5: Escape sequences */
    {
        struct json_stream s;
        json_open_string(&s, "[\"hello\\nworld\"]");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_next(&s);
        if (t != JSON_ARRAY) return 38;

        t = json_next(&s);
        if (t != JSON_STRING) return 39;
        {
            const char *str = json_get_string(&s, 0);
            if (str[5] != '\n') return 40;
        }

        t = json_next(&s);
        if (t != JSON_ARRAY_END) return 43;

        json_close(&s);
    }

    /* Test 6: Empty object and array */
    {
        struct json_stream s;
        json_open_string(&s, "{\"a\": {}, \"b\": []}");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_next(&s);
        if (t != JSON_OBJECT) return 44;

        t = json_next(&s);
        if (t != JSON_STRING) return 45;

        t = json_next(&s);
        if (t != JSON_OBJECT) return 46;
        t = json_next(&s);
        if (t != JSON_OBJECT_END) return 47;

        t = json_next(&s);
        if (t != JSON_STRING) return 48;

        t = json_next(&s);
        if (t != JSON_ARRAY) return 49;
        t = json_next(&s);
        if (t != JSON_ARRAY_END) return 50;

        t = json_next(&s);
        if (t != JSON_OBJECT_END) return 51;

        t = json_next(&s);
        if (t != JSON_DONE) return 52;

        json_close(&s);
    }

    /* Test 7: Number formats */
    {
        struct json_stream s;
        json_open_string(&s, "[0, -5, 3.14, 1e2]");
        json_set_streaming(&s, 0);

        enum json_type t;

        t = json_next(&s);
        if (t != JSON_ARRAY) return 53;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 54;
        if (json_get_number(&s) != 0.0) return 55;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 56;
        if (json_get_number(&s) != -5.0) return 57;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 58;

        t = json_next(&s);
        if (t != JSON_NUMBER) return 59;
        if (json_get_number(&s) != 100.0) return 60;

        t = json_next(&s);
        if (t != JSON_ARRAY_END) return 62;

        json_close(&s);
    }

    /* Test 8: json_get_depth */
    {
        struct json_stream s;
        json_open_string(&s, "{\n  \"key\": [1]\n}");
        json_set_streaming(&s, 0);

        json_next(&s);
        if (json_get_depth(&s) != 1) return 63;

        json_next(&s);
        json_next(&s);
        if (json_get_depth(&s) != 2) return 64;

        json_next(&s);
        json_next(&s);
        json_next(&s);
        if (json_get_depth(&s) != 0) return 65;

        json_close(&s);
    }

    return 42;
}
