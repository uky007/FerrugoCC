struct Point {
    int x;
    int y;
};

struct Point gpa;
struct Point gpb;
struct Point gpc;

int dot(struct Point *a, struct Point *b) {
    return a->x * b->x + a->y * b->y;
}

int mag_sq(struct Point *p) {
    return p->x * p->x + p->y * p->y;
}

int main(void) {
    gpa.x = 3;
    gpa.y = 4;

    gpb.x = 1;
    gpb.y = 2;

    int d = dot(&gpa, &gpb);
    int m = mag_sq(&gpa);

    gpc.x = gpa.x + gpb.x;
    gpc.y = gpa.y + gpb.y;

    return d + m + gpc.x + gpc.y;
}
