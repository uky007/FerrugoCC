int op_add(int a, int b) { return a + b; }
int op_sub(int a, int b) { return a - b; }
int op_mul(int a, int b) { return a * b; }
int op_mod(int a, int b) { return a % b; }

int dispatch(int op, int a, int b) {
    int result = 0;
    if (op == 0)
        result = op_add(a, b);
    else if (op == 1)
        result = op_sub(a, b);
    else if (op == 2)
        result = op_mul(a, b);
    else if (op == 3)
        result = op_mod(a, b);
    return result;
}

int main(void) {
    int r0 = dispatch(0, 10, 3);
    int r1 = dispatch(1, 11, 3);
    int r2 = dispatch(2, 12, 3);
    int r3 = dispatch(3, 13, 3);

    int total = r0 + r1 + r2 + r3;

    int inner = dispatch(1, 20, 5);
    int chain = dispatch(0, inner, 7);

    return total - chain + 24;
}
