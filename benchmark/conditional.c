int main(void) {
    int x = 15;
    int y = 7;
    int result;

    if (x > 20) {
        result = 100;
    } else if (x > 10) {
        if (y < 10) {
            result = 77;
        } else {
            result = 60;
        }
    } else if (x > 5) {
        result = 50;
    } else {
        result = 25;
    }

    if (result > 70 && y == 7) {
        result = result + y - 7;
    }

    return result;
}
