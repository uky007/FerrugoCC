struct Node {
    int value;
    int next;
};

int sum_list(struct Node *nodes, int head) {
    int total = 0;
    int cur = head;
    while (cur != -1) {
        total = total + nodes[cur].value;
        cur = nodes[cur].next;
    }
    return total;
}

int count_list(struct Node *nodes, int head) {
    int cnt = 0;
    int cur = head;
    while (cur != -1) {
        cnt = cnt + 1;
        cur = nodes[cur].next;
    }
    return cnt;
}

int find_max(struct Node *nodes, int head) {
    int mx = nodes[head].value;
    int cur = nodes[head].next;
    while (cur != -1) {
        if (nodes[cur].value > mx)
            mx = nodes[cur].value;
        cur = nodes[cur].next;
    }
    return mx;
}

int main(void) {
    struct Node nodes[8];
    nodes[0].value = 10; nodes[0].next = 5;
    nodes[1].value = 20; nodes[1].next = 4;
    nodes[2].value = 5;  nodes[2].next = 6;
    nodes[3].value = 1;  nodes[3].next = 7;
    nodes[4].value = 8;  nodes[4].next = 0;
    nodes[5].value = 3;  nodes[5].next = 2;
    nodes[6].value = 12; nodes[6].next = -1;
    nodes[7].value = 15; nodes[7].next = 1;

    int head = 3;
    int s = sum_list(nodes, head);
    int c = count_list(nodes, head);
    int mx = find_max(nodes, head);
    return s - c * 10 + mx + 1;
}
