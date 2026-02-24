int a[9];
int b[9];
int c[9];

void mat_mul(int *dst, int *m1, int *m2) {
    int i = 0;
    while (i < 3) {
        int j = 0;
        while (j < 3) {
            int sum = 0;
            int k = 0;
            while (k < 3) {
                sum = sum + m1[i * 3 + k] * m2[k * 3 + j];
                k = k + 1;
            }
            dst[i * 3 + j] = sum;
            j = j + 1;
        }
        i = i + 1;
    }
}

int trace(int *m) {
    return m[0] + m[4] + m[8];
}

int main(void) {
    a[0] = 1; a[1] = 2; a[2] = 0;
    a[3] = 0; a[4] = 1; a[5] = 2;
    a[6] = 2; a[7] = 0; a[8] = 1;

    b[0] = 1; b[1] = 0; b[2] = 2;
    b[3] = 2; b[4] = 1; b[5] = 0;
    b[6] = 0; b[7] = 2; b[8] = 1;

    mat_mul(c, a, b);

    int t = trace(c);

    int d[9];
    mat_mul(d, c, a);
    int t2 = trace(d);

    return t + t2 - 12;
}
