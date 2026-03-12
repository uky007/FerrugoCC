#define JSMN_STATIC
#include "jsmn.h"

int main(void) {
    jsmn_parser parser;
    jsmntok_t tokens[16];

    jsmn_init(&parser);

    const char *json = "{\"key\": \"value\", \"num\": 42}";
    int r = jsmn_parse(&parser, json, 27, tokens, 16);

    // r should be 5 (root object + 4 tokens: key, value, num, 42)
    if (r < 0) return 1;

    // Verify root is an object
    if (tokens[0].type != JSMN_OBJECT) return 2;

    return r - 5 + 42;  // should be 42
}
