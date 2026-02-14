int factorial(int n) {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

int add(int a, int b) {
    return a + b;
}

int sub(int a, int b) {
    return a - b;
}

int main(void) {
    int f5 = factorial(5);
    int s = add(f5, 10);
    int result = sub(s, 10);
    return result;
}
