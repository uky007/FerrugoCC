int classify(int n) {
    if (n < 0) {
        if (n < -100)
            return 1;
        else if (n < -50)
            return 2;
        else if (n < -10)
            return 3;
        else
            return 4;
    } else if (n == 0) {
        return 5;
    } else {
        if (n < 10)
            return 6;
        else if (n < 50)
            return 7;
        else if (n < 100)
            return 8;
        else
            return 9;
    }
}

int score(int cls) {
    if (cls == 1) return 10;
    if (cls == 2) return 8;
    if (cls == 3) return 6;
    if (cls == 4) return 4;
    if (cls == 5) return 0;
    if (cls == 6) return 3;
    if (cls == 7) return 5;
    if (cls == 8) return 7;
    if (cls == 9) return 9;
    return 0;
}

int main(void) {
    int total = 0;
    int values[9];
    values[0] = -200;
    values[1] = -75;
    values[2] = -30;
    values[3] = -5;
    values[4] = 0;
    values[5] = 7;
    values[6] = 25;
    values[7] = 80;
    values[8] = 150;

    int i = 0;
    while (i < 9) {
        int cls = classify(values[i]);
        total = total + score(cls);
        i = i + 1;
    }
    total = total + 11;
    return total;
}
