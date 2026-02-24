int hash(int x, int mod) {
    int h = ((x * 2654435 + 1013904) % mod);
    if (h < 0) h = -h;
    return h;
}

int power_mod(int base, int exp, int mod) {
    int result = 1;
    int b = base % mod;
    while (exp > 0) {
        if (exp % 2 == 1)
            result = (result * b) % mod;
        b = (b * b) % mod;
        exp = exp / 2;
    }
    return result;
}

int digit_sum(int n) {
    int sum = 0;
    if (n < 0) n = -n;
    while (n > 0) {
        sum = sum + n % 10;
        n = n / 10;
    }
    return sum;
}

int main(void) {
    int h1 = hash(42, 100);
    int h2 = hash(123, 100);
    int h3 = hash(7, 100);

    int p1 = power_mod(2, 10, 1000);
    int p2 = power_mod(3, 5, 100);

    int d1 = digit_sum(12345);
    int d2 = digit_sum(99999);

    int total = h1 + h2 + h3 + p1 + p2 + d1 + d2;
    return total - 217;
}
