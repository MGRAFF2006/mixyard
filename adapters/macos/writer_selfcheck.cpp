// SPDX-License-Identifier: MIT
#include "capture_writer.hpp"
#include <cassert>
#include <cstdlib>
#include <memory>

using namespace std::chrono_literals;
static const char* mode;
static std::atomic<uint32_t> blocked{0};

#if defined(__linux__)
// Linker fault injection affects the real WavWriter, without native capture or disk stalls.
extern "C" size_t __real_fwrite(const void*, size_t, size_t, FILE*);
extern "C" int __real_fflush(FILE*);
extern "C" int __real_fseek(FILE*, long, int);
extern "C" int __real_fclose(FILE*);

[[noreturn]] static void stall() {
    blocked.store(1, std::memory_order_release);
    for (;;) std::this_thread::sleep_for(10ms);
}

extern "C" size_t __wrap_fwrite(const void* p, size_t size, size_t count, FILE* file) {
    if (file != stdout && file != stderr) {
        static unsigned headers = 0;
        const bool header = size * count == 56;
        if (header) ++headers;
        if ((std::strcmp(mode, "write-block") == 0 && !header) ||
            (std::strcmp(mode, "header-block") == 0 && header && headers == 2)) stall();
        if (std::strcmp(mode, "write-error") == 0 && !header) return 0;
    }
    return __real_fwrite(p, size, count, file);
}
extern "C" int __wrap_fflush(FILE* file) {
    if (file != stdout && file != stderr) {
        if (std::strcmp(mode, "flush-block") == 0) stall();
        if (std::strcmp(mode, "flush-error") == 0 || std::strcmp(mode, "destructor-block") == 0) return EOF;
    }
    return __real_fflush(file);
}
extern "C" int __wrap_fseek(FILE* file, long offset, int origin) {
    if (std::strcmp(mode, "seek-block") == 0) stall();
    return __real_fseek(file, offset, origin);
}
extern "C" int __wrap_fclose(FILE* file) {
    if (std::strcmp(mode, "close-block") == 0 || std::strcmp(mode, "destructor-block") == 0) stall();
    const int result = __real_fclose(file);
    return std::strcmp(mode, "close-error") == 0 ? EOF : result;
}
#endif

int main(int argc, const char* argv[]) {
    assert(argc == 3);
    mode = argv[1];
    auto queue = std::make_unique<mixyard::CaptureQueue>();
    const float samples[] = {0.25f, -0.25f, 0.5f, -0.5f};
    assert(queue->push(samples, nullptr, 2));
    mixyard::CaptureWriter writer(*queue, argv[2], 48000);
    if (!writer.stopAndWait(2s)) {
        assert(std::strstr(mode, "block") && blocked.load(std::memory_order_acquire));
        // Mirror the probe's timeout path: report failure then skip all destructors.
        std::puts("{\"capabilities\":{\"processCapture\":false},\"diagnostics\":[{\"code\":\"disk-write-timeout\",\"severity\":\"error\"},{\"code\":\"capture-unqualified\",\"severity\":\"error\"}]}");
        std::fflush(stdout);
        std::_Exit(1);
    }
    assert(!std::strstr(mode, "block"));
    if (std::strcmp(mode, "create-error") == 0) {
        assert(!writer.opened && writer.failed.load());
        assert(writer.error.find("Cannot create WAV") != std::string::npos);
        assert(writer.frames == 0 && writer.writtenFrames.load() == 0);
    } else if (std::strcmp(mode, "normal") == 0) {
        assert(writer.opened);
        assert(!writer.failed.load() && writer.error.empty());
        assert(writer.frames == 2 && writer.hasSignal && writer.writtenFrames.load() == 2);
        assert(!queue->front());
    } else {
        assert(writer.opened);
        assert(writer.failed.load() && !writer.error.empty());
        if (std::strcmp(mode, "write-error") == 0) {
            assert(writer.error.find("PCM disk write failed") != std::string::npos);
            assert(writer.frames == 0 && writer.writtenFrames.load() == 0);
        } else {
            assert(writer.frames == 2);
            assert(writer.error.find(std::strcmp(mode, "flush-error") == 0 ? "Cannot finalize WAV" : "WAV close failed") != std::string::npos);
        }
    }
    std::printf("PASS: writer %s completed with expected status; no native capture evidence\n", mode);
}
