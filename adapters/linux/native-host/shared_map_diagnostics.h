// SPDX-License-Identifier: MIT
#ifndef MIXYARD_SHARED_MAP_DIAGNOSTICS_H
#define MIXYARD_SHARED_MAP_DIAGNOSTICS_H
#include <ctype.h>
#include <errno.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "buffer_diagnostics.h"
#define MX_MAP_TARGETS (3 * MX_NATIVE_REGISTRATIONS)
#define MX_MAP_BYTE_LIMIT (1024 * 1024)
#define MX_MAP_RECORD_LIMIT 4096
#define MX_MAP_PAIR_LIMIT 16
// Internal scalar coordinates only; no target address/dev/inode/path is printed.
struct mx_map_target {
    uintptr_t address;
    uint32_t length, region; // 0 PCM capacity, 1 chunk, 2 header
    uint64_t registration, revision, observed_ns;
};
enum mx_backing_kind { MX_BACKING_UNKNOWN, MX_BACKING_PRIVATE, MX_BACKING_ANONYMOUS, MX_BACKING_SHARED_FILE };
struct mx_backing_coordinate {
    uint64_t snapshot, offset, length;
    uint32_t cookie;
    enum mx_backing_kind kind;
    bool covered;
};
enum mx_map_reason { MX_MAP_PENDING, MX_MAP_OK, MX_MAP_OPEN, MX_MAP_READ, MX_MAP_BYTES,
    MX_MAP_RECORDS, MX_MAP_PREFIX, MX_MAP_SYNTAX, MX_MAP_SHORT_LINE, MX_MAP_NO_TARGETS,
    MX_MAP_CHANGED, MX_MAP_CLOCK, MX_MAP_OWNER, MX_MAP_NO_RECORDS };
struct mx_map_snapshot {
    uint64_t id, event_epoch, capture_before_ns, capture_after_ns, maps_before_ns, maps_after_ns, applied_after_ns;
    unsigned count, bytes, records;
    bool maps_complete, targets_stable;
    enum mx_map_reason reason;
    struct mx_map_target targets[MX_MAP_TARGETS];
    struct mx_backing_coordinate coordinates[MX_MAP_TARGETS];
};
struct mx_map_key { uint64_t major, minor, inode; };
static inline void mx_map_unknown(struct mx_map_snapshot *s, enum mx_map_reason reason) {
    s->reason = reason; s->targets_stable = false;
    memset(s->coordinates, 0, sizeof(s->coordinates));
}
static inline const char *mx_map_reason_name(enum mx_map_reason reason) {
    static const char *const names[] = {"pending", "snapshot-only", "open", "read", "byte-limit",
        "record-limit", "prefix-limit", "syntax", "incomplete-line", "no-targets", "target-changed",
        "clock-unknown", "owner-unavailable", "no-records"};
    return (unsigned)reason < sizeof(names)/sizeof(names[0]) ? names[reason] : "unknown";
}
static inline bool mx_map_number(const char **text, int base, uint64_t *value) {
    if (!(base == 16 ? isxdigit((unsigned char)**text) : isdigit((unsigned char)**text))) return false;
    char *end; errno = 0;
    uintmax_t n = strtoumax(*text, &end, base);
    if (errno == ERANGE || end == *text || n > UINT64_MAX) return false;
    *text = end; *value = (uint64_t)n; return true;
}
static inline bool mx_map_space(const char **text) {
    if (**text != ' ' && **text != '\t') return false;
    do ++*text; while (**text == ' ' || **text == '\t');
    return true;
}
static inline bool mx_map_line(const char *prefix, struct mx_map_snapshot *s,
                               struct mx_map_key *keys, unsigned *key_count) {
    const char *p = prefix;
    uint64_t low, high, file_offset, major, minor, inode;
    if (!mx_map_number(&p, 16, &low) || *p++ != '-' || !mx_map_number(&p, 16, &high) ||
        low >= high || low > UINTPTR_MAX || high > UINTPTR_MAX || !mx_map_space(&p)) return false;
    if (strlen(p) < 5 || (p[0] != 'r' && p[0] != '-') || (p[1] != 'w' && p[1] != '-') ||
        (p[2] != 'x' && p[2] != '-') || (p[3] != 's' && p[3] != 'p')) return false;
    bool shared = p[3] == 's'; p += 4;
    if (!mx_map_space(&p) || !mx_map_number(&p, 16, &file_offset) || !mx_map_space(&p) ||
        !mx_map_number(&p, 16, &major) || *p++ != ':' || !mx_map_number(&p, 16, &minor) ||
        major > UINT32_MAX || minor > UINT32_MAX || !mx_map_space(&p) ||
        !mx_map_number(&p, 10, &inode)) return false;
    while (*p == ' ' || *p == '\t') ++p;
    if (*p) return false;
    for (unsigned i = 0; i < s->count; ++i) {
        const struct mx_map_target *t = &s->targets[i];
        if (!t->address || !t->length || t->address < low || t->address >= high || t->length > high-t->address) continue;
        struct mx_backing_coordinate *c = &s->coordinates[i];
        if (c->covered) return false; // Ambiguous/overlapping input mappings are not evidence.
        c->covered = true; c->snapshot = s->id; c->length = t->length;
        if (!inode) { c->kind = MX_BACKING_ANONYMOUS; continue; }
        if (!shared) { c->kind = MX_BACKING_PRIVATE; continue; } // Never infer COW sharing.
        uint64_t relative = t->address-low;
        if (file_offset > UINT64_MAX-relative || file_offset+relative > UINT64_MAX-t->length) continue;
        unsigned key;
        for (key = 0; key < *key_count; ++key)
            if (keys[key].major == major && keys[key].minor == minor && keys[key].inode == inode) break;
        if (key == *key_count) {
            if (*key_count == MX_MAP_TARGETS) return false;
            keys[(*key_count)++] = (struct mx_map_key){major, minor, inode};
        }
        c->kind = MX_BACKING_SHARED_FILE; c->cookie = key+1; c->offset = file_offset+relative;
    }
    return true;
}
// OFF RT only. Read through actual EOF, including short reads, with hard bounds.
// The prefix buffer stops after inode; filename/path bytes are never retained.
static inline bool mx_map_read(FILE *file, struct mx_map_snapshot *s, unsigned byte_limit, unsigned record_limit) {
    s->maps_complete = false; s->bytes = s->records = 0;
    if (!file || !s->id || !s->count || s->count > MX_MAP_TARGETS) { mx_map_unknown(s, MX_MAP_NO_TARGETS); return false; }
    struct mx_map_key keys[MX_MAP_TARGETS]; unsigned key_count = 0;
    memset(s->coordinates, 0, sizeof(s->coordinates));
    char prefix[192]; unsigned used = 0, fields = 0; bool was_space = true, discard = false;
    int ch;
    while ((ch = fgetc(file)) != EOF) {
        if (++s->bytes > byte_limit) { mx_map_unknown(s, MX_MAP_BYTES); return false; }
        if (ch == '\n') {
            if (++s->records > record_limit) { mx_map_unknown(s, MX_MAP_RECORDS); return false; }
            prefix[used] = 0;
            if (!mx_map_line(prefix, s, keys, &key_count)) { mx_map_unknown(s, MX_MAP_SYNTAX); return false; }
            used = fields = 0; was_space = true; discard = false; continue;
        }
        if (discard) continue;
        bool space = ch == ' ' || ch == '\t';
        if (fields == 5 && space) { discard = true; continue; }
        if (!space && was_space) ++fields;
        if (used == sizeof(prefix)-1) { mx_map_unknown(s, MX_MAP_PREFIX); return false; }
        prefix[used++] = (char)ch; was_space = space;
    }
    if (ferror(file)) { mx_map_unknown(s, MX_MAP_READ); return false; }
    if (used || discard) { mx_map_unknown(s, MX_MAP_SHORT_LINE); return false; }
    if (!s->records) { mx_map_unknown(s, MX_MAP_NO_RECORDS); return false; }
    s->maps_complete = true; s->reason = MX_MAP_OK; return true;
}
static inline bool mx_snapshot_overlap(const struct mx_backing_coordinate *a, const struct mx_backing_coordinate *b) {
    return a->snapshot && a->snapshot == b->snapshot && a->kind == MX_BACKING_SHARED_FILE &&
        b->kind == MX_BACKING_SHARED_FILE && a->cookie && a->cookie == b->cookie && a->length && b->length &&
        a->offset <= UINT64_MAX-a->length && b->offset <= UINT64_MAX-b->length &&
        a->offset < b->offset+b->length && b->offset < a->offset+a->length;
}
static inline void mx_print_backing_coordinate(const struct mx_backing_coordinate *c, bool eligible) {
    const char *kind = "unknown";
    if (eligible && c->kind == MX_BACKING_SHARED_FILE) kind = "shared-file-snapshot";
    else if (eligible && c->kind == MX_BACKING_PRIVATE) kind = "private-cow-unknown";
    else if (eligible && c->kind == MX_BACKING_ANONYMOUS) kind = "anonymous-unknown";
    printf("{\"kind\":\"%s\",\"cookie\":", kind);
    if (eligible && c->kind == MX_BACKING_SHARED_FILE) printf("%u,\"fileOffset\":%" PRIu64 ",\"length\":%" PRIu64, c->cookie, c->offset, c->length);
    else printf("null,\"fileOffset\":null,\"length\":null");
    printf("}");
}

// OFF RT serialization only. Candidate relations describe this maps observation;
// even an unchanged target ID/address cannot establish backing lifetime.
static inline bool mx_map_eligible(const struct mx_map_snapshot *s) {
    return s->reason == MX_MAP_OK && s->maps_complete && s->targets_stable;
}
static inline bool mx_map_target_matches(const struct mx_map_target *a, const struct mx_map_target *b) {
    return a->registration == b->registration && a->region == b->region && a->revision == b->revision &&
        a->address == b->address && a->length == b->length;
}
static inline void mx_print_map_snapshot(const struct mx_map_snapshot *s, const uint64_t *selected, unsigned selected_count) {
    bool eligible = mx_map_eligible(s);
    unsigned pairs = 0, covered = 0;
    if (eligible) for (unsigned i = 0; i < s->count; ++i) {
        if (s->coordinates[i].covered) ++covered;
        if (s->targets[i].region != 0) continue;
        for (unsigned j = 0; j < s->count; ++j)
            if (s->targets[j].region && s->targets[i].registration != s->targets[j].registration &&
                mx_snapshot_overlap(&s->coordinates[i], &s->coordinates[j])) ++pairs;
    }
    bool pair_overflow = pairs > MX_MAP_PAIR_LIMIT;
    printf("{\"snapshotGeneration\":%" PRIu64 ",\"scope\":\"snapshot-time-candidate-only\",\"laterCallbackBacking\":\"unknown\","
           "\"state\":\"%s\",\"eventEpoch\":%" PRIu64 ",\"captureBeforeNs\":%" PRIu64 ",\"captureAfterNs\":%" PRIu64
           ",\"mapsBeforeNs\":%" PRIu64 ",\"mapsAfterNs\":%" PRIu64 ",\"applyAfterRecheckNs\":%" PRIu64
           ",\"actualMapsEof\":%s,\"targetAssociationRechecked\":%s,\"mapsBytes\":%u,\"mapsRecords\":%u,\"targetRanges\":%u,\"coveredRanges\":%u,"
           "\"candidateOverflow\":%s,\"sharedFilePairScanComplete\":%s,\"selectedCoverage\":[",
           s->id, mx_map_reason_name(s->reason), s->event_epoch, s->capture_before_ns, s->capture_after_ns,
           s->maps_before_ns, s->maps_after_ns, s->applied_after_ns, s->maps_complete ? "true" : "false",
           eligible ? "true" : "false", s->bytes, s->records, s->count, covered,
           pair_overflow ? "true" : "false", eligible && !pair_overflow ? "true" : "false");
    bool comma = false;
    // At most seven selected IDs: first invalid borrow and six last writers.
    for (unsigned i = 0; i < s->count && selected_count <= 7; ++i) {
        const struct mx_map_target *t = &s->targets[i]; bool keep = false;
        for (unsigned j = 0; j < selected_count; ++j) if (selected[j] && t->registration == selected[j]) keep = true;
        if (!keep) continue;
        printf("%s{\"registrationId\":%" PRIu64 ",\"rangeRevision\":%" PRIu64 ",\"region\":%u,\"lastBorrowObservedNs\":%" PRIu64 ",\"coordinate\":",
            comma ? "," : "", t->registration, t->revision, t->region, t->observed_ns);
        comma = true; mx_print_backing_coordinate(&s->coordinates[i], eligible); printf("}");
    }
    printf("],\"selectedMissingRegistrationIds\":["); comma = false;
    for (unsigned i = 0; i < selected_count && selected_count <= 7; ++i) {
        if (!selected[i]) continue;
        bool found = false;
        for (unsigned j = 0; j < s->count; ++j) if (s->targets[j].registration == selected[i]) found = true;
        if (!found) { printf("%s%" PRIu64, comma ? "," : "", selected[i]); comma = true; }
    }
    printf("],\"crossBufferCandidates\":["); comma = false;
    // Overflow returns unknown instead of emitting a truncated complete relation.
    if (eligible && !pair_overflow) for (unsigned i = 0; i < s->count; ++i) {
        const struct mx_backing_coordinate *a = &s->coordinates[i];
        if (s->targets[i].region != 0) continue;
        for (unsigned j = 0; j < s->count; ++j) {
            const struct mx_backing_coordinate *b = &s->coordinates[j];
            if (!s->targets[j].region || s->targets[i].registration == s->targets[j].registration || !mx_snapshot_overlap(a, b)) continue;
            uint64_t start = a->offset > b->offset ? a->offset : b->offset;
            uint64_t end = a->offset+a->length < b->offset+b->length ? a->offset+a->length : b->offset+b->length;
            printf("%s{\"pcmRegistrationId\":%" PRIu64 ",\"metadataRegistrationId\":%" PRIu64 ",\"metadataRegion\":%u,\"cookie\":%u,\"fileOffset\":%" PRIu64 ",\"length\":%" PRIu64 "}",
                comma ? "," : "", s->targets[i].registration, s->targets[j].registration, s->targets[j].region,
                a->cookie, start, end-start); comma = true;
        }
    }
    printf("]}");
}

#endif
