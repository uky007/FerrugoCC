struct Vec2 {
    int x;
    int y;
};

struct Rect {
    struct Vec2 origin;
    struct Vec2 size;
};

struct Layout {
    struct Rect bounds;
    int padding;
    int visible;
};

int vec2_dot(struct Vec2 *a, struct Vec2 *b) {
    return a->x * b->x + a->y * b->y;
}

int rect_area(struct Rect *r) {
    return r->size.x * r->size.y;
}

int rect_perimeter(struct Rect *r) {
    return 2 * (r->size.x + r->size.y);
}

int layout_inner_area(struct Layout *l) {
    int w = l->bounds.size.x - 2 * l->padding;
    int h = l->bounds.size.y - 2 * l->padding;
    if (w < 0) w = 0;
    if (h < 0) h = 0;
    return w * h;
}

int main(void) {
    struct Layout panels[2];

    panels[0].bounds.origin.x = 0;
    panels[0].bounds.origin.y = 0;
    panels[0].bounds.size.x = 10;
    panels[0].bounds.size.y = 8;
    panels[0].padding = 1;
    panels[0].visible = 1;

    panels[1].bounds.origin.x = 10;
    panels[1].bounds.origin.y = 0;
    panels[1].bounds.size.x = 6;
    panels[1].bounds.size.y = 8;
    panels[1].padding = 2;
    panels[1].visible = 1;

    int total_area = 0;
    int total_inner = 0;
    int i = 0;
    while (i < 2) {
        if (panels[i].visible) {
            total_area = total_area + rect_area(&panels[i].bounds);
            total_inner = total_inner + layout_inner_area(&panels[i]);
        }
        i = i + 1;
    }

    int perim = rect_perimeter(&panels[0].bounds);

    struct Vec2 v1;
    v1.x = 3;
    v1.y = 4;
    struct Vec2 v2;
    v2.x = 4;
    v2.y = 3;
    int dot = vec2_dot(&v1, &v2);

    return total_inner - perim + dot + 33;
}
