int my_strlen(char *s) {
    int len = 0;
    while (s[len] != 0)
        len = len + 1;
    return len;
}

int my_strstr(char *haystack, char *needle) {
    int hlen = my_strlen(haystack);
    int nlen = my_strlen(needle);
    if (nlen == 0) return 0;
    if (nlen > hlen) return -1;

    int i = 0;
    while (i <= hlen - nlen) {
        int j = 0;
        while (j < nlen && haystack[i + j] == needle[j])
            j = j + 1;
        if (j == nlen)
            return i;
        i = i + 1;
    }
    return -1;
}

int count_occurrences(char *haystack, char *needle) {
    int count = 0;
    int hlen = my_strlen(haystack);
    int nlen = my_strlen(needle);
    if (nlen == 0) return 0;

    int i = 0;
    while (i <= hlen - nlen) {
        int j = 0;
        while (j < nlen && haystack[i + j] == needle[j])
            j = j + 1;
        if (j == nlen) {
            count = count + 1;
            i = i + nlen;
        } else {
            i = i + 1;
        }
    }
    return count;
}

int main(void) {
    char *text = "abcabcabcabc";
    char *pat1 = "abc";
    char *pat2 = "cab";
    char *pat3 = "xyz";

    int pos1 = my_strstr(text, pat1);
    int cnt1 = count_occurrences(text, pat1);
    int pos2 = my_strstr(text, pat2);
    int cnt2 = count_occurrences(text, pat2);
    int pos3 = my_strstr(text, pat3);

    return pos1 + cnt1 + pos2 + cnt2 + pos3 - 5;
}
