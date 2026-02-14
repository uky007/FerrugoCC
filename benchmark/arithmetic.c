int main(void) {
    int a = 10;
    int b = 3;
    int c = a * b;
    int d = c + 5;
    int e = d - a;
    long f = (long)e;
    int result = (int)(f + (long)b + 2L);
    return result;
}
