int garr[7];

int main(void) {
    garr[0] = 64;
    garr[1] = 34;
    garr[2] = 25;
    garr[3] = 12;
    garr[4] = 22;
    garr[5] = 11;
    garr[6] = 90;

    int n = 7;
    for (int i = 0; i < n - 1; i = i + 1) {
        for (int j = 0; j < n - 1 - i; j = j + 1) {
            if (garr[j] > garr[j + 1]) {
                int tmp = garr[j];
                garr[j] = garr[j + 1];
                garr[j + 1] = tmp;
            }
        }
    }

    return garr[0] + garr[6];
}
