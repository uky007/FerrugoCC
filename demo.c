int strcmp(char *s1, char *s2);
int printf(char *fmt, ...);

int check_password(char *input) {
    char secret[8];
    secret[0] = 's';
    secret[1] = '3';
    secret[2] = 'c';
    secret[3] = 'r';
    secret[4] = '3';
    secret[5] = 't';
    secret[6] = '!';
    secret[7] = 0;

    if (strcmp(input, secret) == 0) {
        printf("Access granted\n");
        return 1;
    }
    printf("Access denied\n");
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        printf("Usage: demo <password>\n");
        return 1;
    }
    return check_password(argv[1]);
}
