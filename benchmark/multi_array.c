int mat[16];
int trans[16];

void fill_matrix(void) {
    int val = 1;
    int i = 0;
    while (i < 4) {
        int j = 0;
        while (j < 4) {
            mat[i * 4 + j] = val;
            val = val + 1;
            j = j + 1;
        }
        i = i + 1;
    }
}

void transpose(void) {
    int i = 0;
    while (i < 4) {
        int j = 0;
        while (j < 4) {
            trans[i * 4 + j] = mat[j * 4 + i];
            j = j + 1;
        }
        i = i + 1;
    }
}

int main(void) {
    fill_matrix();
    transpose();

    int d1 = mat[0] + mat[5] + mat[10] + mat[15];

    int row_sum = mat[0] + mat[1] + mat[2] + mat[3];

    int m4 = mat[4];
    int t1 = trans[1];
    int check = 0;
    if (m4 == t1)
        check = 1;

    return d1 + row_sum + check;
}
