// SPDX-License-Identifier: MIT
// CPU-only helper checks; no PipeWire, SDK, devices or other-process inspection.
#include <assert.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>
#include "shared_map_diagnostics.h"
static bool read_text(struct mx_map_snapshot *s, const char *text, unsigned bytes, unsigned records) {
    FILE *f = fmemopen((void *)text, strlen(text), "r"); assert(f);
    bool result = mx_map_read(f, s, bytes, records); assert(fclose(f) == 0); return result;
}
static struct mx_map_snapshot model(void) {
    struct mx_map_snapshot s = { .id = 1, .count = 4 };
    const uintptr_t addresses[] = {0x1040, 0x3040, 0x5040, 0x7040};
    for (unsigned i = 0; i < 4; ++i) s.targets[i] = (struct mx_map_target){
        .address = addresses[i], .length = 16, .registration = i+1, .revision = 1 };
    return s;
}
static void check_model(void) {
    const char *text = "1000-2000 rw-s 0000 00:01 9 PATH_MUST_NOT_BE_RETAINED\n"
                       "3000-4000 rw-s 0000 00:01 9\n"
                       "5000-6000 rw-p 0000 00:01 9\n"
                       "7000-8000 rw-p 0000 00:00 0\n";
    struct mx_map_snapshot s = model();
    assert(read_text(&s, text, MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    assert(s.maps_complete && s.records == 4 && s.bytes == strlen(text));
    assert(mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[1]));
    assert(s.coordinates[0].offset == 64 && s.coordinates[0].length == 16);
    assert(s.coordinates[2].kind == MX_BACKING_PRIVATE && !mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[2]));
    assert(s.coordinates[3].kind == MX_BACKING_ANONYMOUS && !mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[3]));
    struct mx_backing_coordinate newer = s.coordinates[1]; newer.snapshot++;
    assert(!mx_snapshot_overlap(&s.coordinates[0], &newer));
    newer = s.coordinates[1]; newer.offset = UINT64_MAX-8;
    assert(!mx_snapshot_overlap(&s.coordinates[0], &newer));
    assert(!read_text(&s, text, 8, MX_MAP_RECORD_LIMIT) && s.reason == MX_MAP_BYTES);
    assert(!s.coordinates[0].cookie);
    assert(!read_text(&s, text, MX_MAP_BYTE_LIMIT, 1) && s.reason == MX_MAP_RECORDS);
    assert(!read_text(&s, "", MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT) && s.reason == MX_MAP_NO_RECORDS);
    s = model(); s.targets[0].address = 0x1ff8;
    assert(read_text(&s, text, MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    assert(!s.coordinates[0].covered); // A range straddling VMAs is unknown, never truncated.
    s = model();
    assert(!read_text(&s, "1000-2000 rw-s 0 00:01 9", MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT) && s.reason == MX_MAP_SHORT_LINE);
    assert(!read_text(&s, "1000-2000 rw-s 0 00:01 9\n1000-2000 rw-s 0 00:01 9\n", MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    assert(!read_text(&s, "1000-2000 rw-s 10000000000000000 00:01 9\n", MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    assert(read_text(&s, "1000-2000 rw-s fffffffffffffff0 00:01 9\n", MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    assert(s.coordinates[0].kind == MX_BACKING_UNKNOWN); // Checked file-offset overflow, not wrapped evidence.
    s = model(); assert(read_text(&s, text, MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    struct mx_map_target same = s.targets[0];
    assert(mx_map_target_matches(&same, &s.targets[0]));
    ++same.revision; assert(!mx_map_target_matches(&same, &s.targets[0]));
    same = s.targets[0]; ++same.registration; assert(!mx_map_target_matches(&same, &s.targets[0]));
    same = s.targets[0]; ++same.address; assert(!mx_map_target_matches(&same, &s.targets[0]));
    mx_map_unknown(&s, MX_MAP_CHANGED);
    assert(!mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[1]));
    puts("{\"mode\":\"model\",\"bounds\":true,\"unknownOnChange\":true,\"crossSnapshotRejected\":true}");
}
static void observe_self(struct mx_map_snapshot *s) {
    FILE *f = fopen("/proc/self/maps", "re"); assert(f);
    assert(mx_map_read(f, s, MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT)); assert(fclose(f) == 0);
}
static void check_own_maps(void) {
    int fd = memfd_create("mixyard-owned-backing-check", MFD_CLOEXEC); assert(fd >= 0);
    assert(ftruncate(fd, 4096) == 0);
    unsigned char *a = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    unsigned char *b = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    unsigned char *p = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_PRIVATE, fd, 0);
    unsigned char *n = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    assert(a != MAP_FAILED && b != MAP_FAILED && p != MAP_FAILED && n != MAP_FAILED && a != b);
    struct mx_map_snapshot s = { .id = 10, .count = 4 };
    void *regions[] = {a+64, b+64, p+64, n+64};
    for (unsigned i = 0; i < 4; ++i) s.targets[i] = (struct mx_map_target){
        .address = (uintptr_t)regions[i], .length = 16, .registration = i+1, .revision = 1 };
    observe_self(&s);
    assert(mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[1]));
    assert(!mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[2]) && s.coordinates[2].kind == MX_BACKING_PRIVATE);
    assert(!mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[3]) && s.coordinates[3].kind == MX_BACKING_ANONYMOUS);
    memset(a+64, 'S', 16); assert(!memcmp(a+64, b+64, 16));
    for (unsigned i = 0; i < 32; ++i) assert(a[i] == 0); // Own disjoint metadata region unchanged.
    memset(p+64, 'P', 16); for (unsigned i = 0; i < 16; ++i) assert(a[64+i] == 'S');
    struct mx_buffer_ids live = {0}; assert(mx_buffer_register(&live, b, 2));
    assert(mx_buffer_unregister(&live, b) == 2); assert(munmap(b, 4096) == 0);
    void *reused = mmap(b, 4096, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE, -1, 0);
    assert(reused == b); // Never overwrite a racing mapping.
    assert(!mx_buffer_identity(&live, b)); assert(mx_buffer_register(&live, reused, 5));
    struct mx_map_snapshot fresh = { .id = 11, .count = 1 };
    fresh.targets[0] = (struct mx_map_target){ .address = (uintptr_t)reused+64, .length = 16, .registration = 5, .revision = 1 };
    observe_self(&fresh); assert(fresh.coordinates[0].kind == MX_BACKING_ANONYMOUS);
    assert(!mx_snapshot_overlap(&s.coordinates[0], &fresh.coordinates[0]));
    assert(mx_buffer_unregister(&live, reused) == 5);
    assert(munmap(reused, 4096) == 0 && munmap(a, 4096) == 0 && munmap(p, 4096) == 0 && munmap(n, 4096) == 0);
    assert(close(fd) == 0);
    puts("{\"mode\":\"own-maps\",\"sharedViewsAlias\":true,\"privateCowIsolated\":true,\"retiredReuseRejected\":true}");
}
static void check_serialization(void) {
    struct mx_map_snapshot s = model();
    const char *text = "1000-2000 rw-s 0000 00:01 9 PRIVATE_PATH\n"
                       "3000-4000 rw-s 0000 00:01 9\n"
                       "5000-6000 rw-p 0000 00:01 9\n"
                       "7000-8000 rw-p 0000 00:00 0\n";
    assert(read_text(&s, text, MX_MAP_BYTE_LIMIT, MX_MAP_RECORD_LIMIT));
    s.targets[1].region = 1; s.targets_stable = true;
    uint64_t selected[] = {1,2,3,4};
    mx_print_map_snapshot(&s, selected, 4); putchar('\n');
    mx_map_unknown(&s, MX_MAP_CHANGED);
    mx_print_map_snapshot(&s, selected, 4); putchar('\n');
    s = (struct mx_map_snapshot){ .id = 2, .count = 21,
        .reason = MX_MAP_OK, .maps_complete = true, .targets_stable = true };
    for (unsigned i = 0; i < s.count; ++i) {
        s.targets[i] = (struct mx_map_target){ .registration = i/3+1, .region = i%3, .revision = 1 };
        s.coordinates[i] = (struct mx_backing_coordinate){ .snapshot = s.id, .offset = 64,
            .length = 16, .cookie = 1, .kind = MX_BACKING_SHARED_FILE, .covered = true };
    }
    uint64_t all_selected[] = {1,2,3,4,5,6,7};
    mx_print_map_snapshot(&s, all_selected, 7); putchar('\n');
}
static void check_selected_pairs(void) {
    struct mx_map_snapshot s = { .id = 3, .count = 64, .reason = MX_MAP_OK,
        .maps_complete = true, .targets_stable = true };
    for (unsigned i = 0; i < s.count; ++i) {
        s.targets[i] = (struct mx_map_target){ .registration = i+1, .region = i ? 1 : 0, .revision = 1 };
        s.coordinates[i] = (struct mx_backing_coordinate){ .snapshot = s.id, .offset = 64,
            .length = 16, .cookie = 1, .kind = MX_BACKING_SHARED_FILE, .covered = true };
    }
    uint64_t selected[] = {1,2};
    mx_print_map_snapshot(&s, selected, 2); putchar('\n');
}
static void check_nonzero_offset(void) {
    long page = sysconf(_SC_PAGESIZE); assert(page >= 256 && page <= 65536);
    int fd = memfd_create("mixyard-owned-coordinate-readback", MFD_CLOEXEC); assert(fd >= 0);
    assert(ftruncate(fd, 3*page) == 0);
    unsigned char expected[16], oracle[16]; memset(expected, 0x35, sizeof(expected));
    assert(pwrite(fd, expected, sizeof(expected), page+64) == (ssize_t)sizeof(expected));
    assert(pread(fd, oracle, sizeof(oracle), page+64) == (ssize_t)sizeof(oracle));
    unsigned char *a = mmap(NULL, (size_t)page, PROT_READ, MAP_SHARED, fd, page);
    unsigned char *b = mmap(NULL, (size_t)page, PROT_READ, MAP_SHARED, fd, page);
    unsigned char *p = mmap(NULL, (size_t)page, PROT_READ, MAP_PRIVATE, fd, page);
    assert(a != MAP_FAILED && b != MAP_FAILED && p != MAP_FAILED && a != b);
    assert(!memcmp(a+64, oracle, sizeof(oracle)) && !memcmp(b+64, oracle, sizeof(oracle)));
    struct mx_map_snapshot s = { .id = 12, .count = 3 };
    void *views[] = {a+64,b+64,p+64};
    for (unsigned i = 0; i < 3; ++i) s.targets[i] = (struct mx_map_target){
        .address = (uintptr_t)views[i], .length = 16, .registration = i+1, .revision = 1, .region = i ? 1 : 0 };
    observe_self(&s);
    assert(s.coordinates[0].offset == (uint64_t)page+64 && s.coordinates[1].offset == (uint64_t)page+64);
    assert(mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[1]));
    assert(s.coordinates[2].kind == MX_BACKING_PRIVATE && !mx_snapshot_overlap(&s.coordinates[0], &s.coordinates[2]));
    assert(munmap(a,(size_t)page) == 0 && munmap(b,(size_t)page) == 0 && munmap(p,(size_t)page) == 0);
    assert(close(fd) == 0);
    puts("{\"mode\":\"nonzero-offset\",\"preadMatches\":true,\"fileOffsetIncludesMapOrigin\":true,\"privateUnknown\":true}");
}
int main(int argc, char **argv) {
    assert(argc == 2);
    if (!strcmp(argv[1], "selected-pairs")) check_selected_pairs();
    else if (!strcmp(argv[1], "nonzero-offset")) check_nonzero_offset();
    else if (!strcmp(argv[1], "serialization")) check_serialization();
    else if (!strcmp(argv[1], "model")) check_model();
    else { assert(!strcmp(argv[1], "own-maps")); check_own_maps(); }
    return 0;
}
