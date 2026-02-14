struct Stats {
    int count;
    int sum;
    int min;
    int max;
};

int gdata[5];
struct Stats gst;

int abs_val(int x) {
    if (x < 0) {
        return -x;
    }
    return x;
}

int gcd(int a, int b) {
    if (b == 0) {
        return a;
    }
    return gcd(b, a % b);
}

int my_strlen(char *s) {
    int len = 0;
    while (s[len] != 0) {
        len = len + 1;
    }
    return len;
}

int main(void) {
    gdata[0] = 12;
    gdata[1] = 7;
    gdata[2] = 25;
    gdata[3] = 3;
    gdata[4] = 18;

    gst.count = 5;
    gst.sum = 0;
    gst.min = gdata[0];
    gst.max = gdata[0];
    for (int i = 0; i < gst.count; i = i + 1) {
        gst.sum = gst.sum + gdata[i];
        if (gdata[i] < gst.min) {
            gst.min = gdata[i];
        }
        if (gdata[i] > gst.max) {
            gst.max = gdata[i];
        }
    }

    int g = gcd(gst.sum, gst.max);
    int a = abs_val(gst.min - gst.max);

    char *label = "bench";
    int slen = my_strlen(label);

    return g + a + slen + gst.count;
}
