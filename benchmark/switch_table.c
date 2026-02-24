enum Color { RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA, WHITE, BLACK };

int color_value(enum Color c) {
    switch (c) {
        case RED:     return 1;
        case GREEN:   return 2;
        case BLUE:    return 4;
        case YELLOW:  return 3;
        case CYAN:    return 6;
        case MAGENTA: return 5;
        case WHITE:   return 7;
        case BLACK:   return 0;
        default:      return -1;
    }
}

int mix(enum Color a, enum Color b) {
    int va = color_value(a);
    int vb = color_value(b);
    return va + vb;
}

int main(void) {
    int total = 0;
    total = total + color_value(RED);
    total = total + color_value(GREEN);
    total = total + color_value(BLUE);
    total = total + color_value(YELLOW);
    total = total + color_value(CYAN);
    total = total + color_value(MAGENTA);
    total = total + color_value(WHITE);
    total = total + color_value(BLACK);
    total = total + mix(RED, BLUE);
    total = total + mix(GREEN, YELLOW);
    total = total + mix(CYAN, MAGENTA);
    total = total + mix(WHITE, BLACK);
    total = total - 1;
    return total;
}
