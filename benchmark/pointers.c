int parr[5];

int main(void) {
    parr[0] = 10;
    parr[1] = 20;
    parr[2] = 30;
    parr[3] = 40;
    parr[4] = 50;

    int *first = &parr[0];
    int *last = &parr[4];
    int sum = *first + *last;

    int *mid = &parr[2];
    sum = sum + *mid;

    *mid = *mid * 2;

    int total = 0;
    for (int i = 0; i < 5; i = i + 1) {
        total = total + parr[i];
    }

    return total - sum;
}
