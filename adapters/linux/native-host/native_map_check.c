// SPDX-License-Identifier: MIT
// Hosted-only public API readback of fresh owned synthetic storage. No context,
// daemon, devices, interposition, writes through mappings or graph gate changes.
#define _GNU_SOURCE
#include <ctype.h>
#include <dlfcn.h>
#include <elf.h>
#include <errno.h>
#include <link.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <pipewire/mem.h>
#include <pipewire/version.h>
#include <spa/buffer/buffer.h>
struct library_identity { uintptr_t base; char build_id[129]; };
typedef struct pw_memmap *(*mapping_api)(struct pw_memblock *, enum pw_memmap_flags, uint32_t, uint32_t, uint32_t *);
static mapping_api map_region; // The resolved public API itself; no replacement/interposition.
// Read only ELF identity metadata exposed by the loader for the resolved API.
// No paths, addresses or application-memory contents are reported.
static int identify_library(struct dl_phdr_info *info, size_t size, void *userdata) {
    (void)size;
    struct library_identity *identity = userdata;
    if ((uintptr_t)info->dlpi_addr != identity->base || info->dlpi_phnum > 128) return 0;
    for (unsigned i = 0; i < info->dlpi_phnum; ++i) {
        const ElfW(Phdr) *note = &info->dlpi_phdr[i];
        if (note->p_type != PT_NOTE || note->p_memsz > 65536 || note->p_memsz < sizeof(ElfW(Nhdr)) ||
            note->p_vaddr > UINTPTR_MAX-identity->base) continue;
        uintptr_t start = identity->base+note->p_vaddr;
        if (note->p_memsz > UINTPTR_MAX-start) continue;
        bool readable = false;
        for (unsigned j = 0; j < info->dlpi_phnum; ++j) {
            const ElfW(Phdr) *load = &info->dlpi_phdr[j];
            if (load->p_type != PT_LOAD || !(load->p_flags & PF_R) || load->p_vaddr > UINTPTR_MAX-identity->base) continue;
            uintptr_t low = identity->base+load->p_vaddr;
            if (load->p_memsz <= UINTPTR_MAX-low && low <= start && start+note->p_memsz <= low+load->p_memsz) readable = true;
        }
        if (!readable) continue;
        const unsigned char *bytes = (const unsigned char *)start;
        size_t at = 0;
        while (note->p_memsz-at >= sizeof(ElfW(Nhdr))) {
            ElfW(Nhdr) header; memcpy(&header, bytes+at, sizeof(header)); at += sizeof(header);
            uint64_t name_size = ((uint64_t)header.n_namesz+3)&~UINT64_C(3);
            uint64_t desc_size = ((uint64_t)header.n_descsz+3)&~UINT64_C(3);
            if (name_size > note->p_memsz-at || desc_size > note->p_memsz-at-name_size) break;
            if (header.n_type == NT_GNU_BUILD_ID && header.n_namesz == 4 && !memcmp(bytes+at, "GNU", 4) &&
                header.n_descsz && header.n_descsz <= 64) {
                for (unsigned b = 0; b < header.n_descsz; ++b)
                    snprintf(identity->build_id+2*b, 3, "%02x", bytes[at+name_size+b]);
                return 1;
            }
            at += (size_t)(name_size+desc_size);
        }
    }
    return 0;
}
static bool safe_version(const char *version) {
    if (!version || !*version || strnlen(version, 64) == 64) return false;
    for (const unsigned char *p = (const unsigned char *)version; *p; ++p)
        if (!isalnum(*p) && *p != '.' && *p != '-' && *p != '+' && *p != '_') return false;
    return true;
}
struct read_case { const char *name; uint32_t offset, cover_offset, cover_size; };
static bool read_case(struct pw_memblock *block, const struct read_case *test, bool comma) {
    enum { WIDTH = 32 };
    unsigned char expected[WIDTH];
    bool oracle = pread(block->fd, expected, WIDTH, test->offset) == WIDTH;
    struct pw_memmap *cover = test->cover_size ? map_region(block, PW_MEMMAP_FLAG_READ,
        test->cover_offset, test->cover_size, NULL) : NULL;
    struct pw_memmap *part = (!test->cover_size || cover) ? map_region(block, PW_MEMMAP_FLAG_READ,
        test->offset, WIDTH, NULL) : NULL;
    bool metadata = part && part->ptr && part->offset == test->offset && part->size == WIDTH;
    bool owned_range = metadata;
    if (cover && metadata) {
        uintptr_t low = (uintptr_t)cover->ptr, ptr = (uintptr_t)part->ptr;
        owned_range = low && cover->size >= WIDTH && ptr >= low && ptr-low <= cover->size-WIDTH;
    }
    bool equal = oracle && owned_range && !memcmp(expected, part->ptr, WIDTH);
    bool cleaned = true;
    if (part && pw_memmap_free(part) < 0) cleaned = false;
    if (cover && pw_memmap_free(cover) < 0) cleaned = false;
    printf("%s{\"name\":\"%s\",\"requestedOffset\":%u,\"requestedBytes\":%u,\"coverOrigin\":%u,\"coverBytes\":%u,"
        "\"preadComplete\":%s,\"returnedMetadataMatches\":%s,\"withinOwnedMapping\":%s,\"readbackMatchesPread\":%s,\"cleanupComplete\":%s}",
        comma ? "," : "", test->name, test->offset, WIDTH, test->cover_offset, test->cover_size,
        oracle ? "true" : "false", metadata ? "true" : "false", owned_range ? "true" : "false", equal ? "true" : "false",
        cleaned ? "true" : "false");
    return equal && cleaned;
}
int main(void) {
    const char *actions = getenv("GITHUB_ACTIONS"), *runner = getenv("RUNNER_ENVIRONMENT");
    if (!actions || strcmp(actions,"true") || !runner || strcmp(runner,"github-hosted")) {
        fputs("mapping readback requires disposable hosted SDK runner\n", stderr); return 2;
    }
    const char *headers = pw_get_headers_version(), *runtime = pw_get_library_version();
    Dl_info owner = {0}; struct library_identity identity = {0};
    // Resolve and identify the exact public function that the readback calls.
    void *symbol = dlsym(RTLD_DEFAULT, "pw_memblock_map");
    _Static_assert(sizeof(map_region) == sizeof(symbol), "POSIX function pointer representation required");
    if (!safe_version(headers) || !safe_version(runtime) || !symbol || !dladdr(symbol, &owner)) {
        fputs("public mapping API/version identity unavailable\n", stderr); return 2;
    }
    memcpy(&map_region, &symbol, sizeof(map_region));
    identity.base = (uintptr_t)owner.dli_fbase;
    if (!dl_iterate_phdr(identify_library, &identity) || !*identity.build_id) {
        fputs("resolved public API build identity unavailable\n", stderr); return 2;
    }
    long page = sysconf(_SC_PAGESIZE);
    if (page < 256 || page > 65536 || (page & (page-1))) {
        fputs("unsupported page size for bounded mapping readback\n", stderr); return 2;
    }
    uint32_t p = (uint32_t)page, allocation = 8*p;
    struct pw_mempool *pool = pw_mempool_new(NULL);
    struct pw_memblock *block = pool ? pw_mempool_alloc(pool, PW_MEMBLOCK_FLAG_READWRITE | PW_MEMBLOCK_FLAG_SEAL,
        SPA_DATA_MemFd, allocation) : NULL;
    if (!block) {
        fputs("fresh owned mapping-readback allocation failed\n", stderr);
        if (pool) pw_mempool_destroy(pool);
        return 2;
    }
    unsigned char tags[256]; bool filled = true;
    for (uint32_t offset = 0; filled && offset < allocation; offset += sizeof(tags)) {
        for (unsigned j = 0; j < sizeof(tags); ++j) tags[j] = (unsigned char)(((offset+j)/64)%251);
        filled = pwrite(block->fd, tags, sizeof(tags), offset) == (ssize_t)sizeof(tags);
    }
    if (!filled) {
        fputs("normal file-I/O marker initialization incomplete\n", stderr);
        pw_mempool_destroy(pool); return 2;
    }
    struct read_case cases[] = {
        {"zero-fresh",0,0,0}, {"page-offset-fresh",p+128,0,0},
        {"page-offset-cover-zero",p+128,0,4*p}, {"nonzero-origin-fresh",3*p+128,0,0},
        {"nonzero-origin-same-page",2*p+128,2*p,4*p}, {"nonzero-origin-later-page",3*p+128,2*p,4*p}
    };
    printf("{\"publicMappingReadback\":true,\"headerVersion\":\"%s\",\"runtimeVersion\":\"%s\",\"resolvedApiLibraryBuildId\":\"%s\","
        "\"pageSize\":%u,\"ownedAllocationBytes\":%u,\"observed4224ShapeIncluded\":%s,\"cases\":[",
        headers,runtime,identity.build_id,p,allocation,p == 4096 ? "true" : "false");
    bool pass = true;
    for (unsigned i = 0; i < sizeof(cases)/sizeof(cases[0]); ++i) if (!read_case(block,&cases[i],i != 0)) pass = false;
    if (p == 4096) {
        struct read_case observed[] = {{"observed-4224-fresh",4224,0,0},{"observed-4224-cover",4224,0,4*p}};
        for (unsigned i = 0; i < 2; ++i) if (!read_case(block,&observed[i],true)) pass = false;
    }
    pw_mempool_destroy(pool);
    printf("],\"allCasesPassed\":%s}\n",pass ? "true" : "false");
    return pass ? 0 : 1; // A mismatch is a failing hosted gate, never an accepted exception.
}
