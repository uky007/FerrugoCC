/* Benchmark 17: ビット演算 — hash, xorshift, bit counting */

int xorshift32(unsigned int state) {
    state = state ^ (state << 13);
    state = state ^ (state >> 17);
    state = state ^ (state << 5);
    return state;
}

int bit_count(unsigned int x) {
    int count = 0;
    while (x != 0u) {
        count = count + (x & 1u);
        x = x >> 1;
    }
    return count;
}

int hash_mix(int a, int b) {
    int h = a ^ (b << 3);
    h = h ^ (h >> 7);
    h = h & 0xFF;
    return h;
}

int ip_pack(int o1, int o2, int o3, int o4) {
    return (o1 << 24) | (o2 << 16) | (o3 << 8) | o4;
}

int ip_octet(int ip, int n) {
    return (ip >> (n * 8)) & 0xFF;
}

int main(void) {
    /* xorshift: non-trivial output */
    unsigned int rng = 12345u;
    rng = xorshift32(rng);
    int r1 = (rng != 0u) ? 1 : 0;  /* 1 */

    /* bit_count(0xFF) = 8 */
    int bc = bit_count(0xFFu);  /* 8 */

    /* hash_mix */
    int h1 = hash_mix(42, 17);  /* deterministic */
    int h2 = hash_mix(99, 3);

    /* IP pack/unpack roundtrip: 10.0.0.1 */
    int ip = ip_pack(10, 0, 0, 1);
    int o1 = ip_octet(ip, 3);  /* 10 */
    int o4 = ip_octet(ip, 0);  /* 1 */

    /* XOR encrypt/decrypt roundtrip */
    int data = 100;
    int key = 0x5A;
    int enc = data ^ key;
    int dec = enc ^ key;  /* 100 */

    /* compound assign */
    int x = 0xFF;
    x &= 0x0F;   /* 15 */
    x |= 0x30;   /* 63 */
    x ^= 0x22;   /* 29 */
    x <<= 1;     /* 58 - 16 = 42 */
    x = x - 16;

    return r1 + bc + (o1 - 10) + (o4 - 1) + (dec - 100) + x - 9;
    /* 1 + 8 + 0 + 0 + 0 + 42 - 9 = 42 */
}
