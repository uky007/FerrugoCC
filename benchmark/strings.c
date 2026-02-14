int my_strlen(char *s) {
    int len = 0;
    while (s[len] != 0) {
        len = len + 1;
    }
    return len;
}

int main(void) {
    char *msg = "Hello";
    int len = my_strlen(msg);
    int c0 = msg[0];
    int c4 = msg[4];
    return c4 - c0 + len;
}
