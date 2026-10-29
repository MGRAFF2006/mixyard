// SPDX-License-Identifier: MIT
#include "host_link.hpp"
#include <cassert>
#include <cstdio>
#include <fstream>
#include <iterator>

static bool realtime = false;
static unsigned heapOperations = 0;
extern "C" void* __real_malloc(size_t);
extern "C" void* __real_calloc(size_t, size_t);
extern "C" void* __real_realloc(void*, size_t);
extern "C" void __real_free(void*);
extern "C" void* __wrap_malloc(size_t n) { if (realtime) ++heapOperations; return __real_malloc(n); }
extern "C" void* __wrap_calloc(size_t n, size_t s) { if (realtime) ++heapOperations; return __real_calloc(n, s); }
extern "C" void* __wrap_realloc(void* p, size_t n) { if (realtime) ++heapOperations; return __real_realloc(p, n); }
extern "C" void __wrap_free(void* p) { if (realtime) ++heapOperations; __real_free(p); }
namespace m = mixyard::macos;
namespace w = mixyard::worker;
namespace {
template<class F> void rejects(F f) {
    bool rejected = false;
    try { f(); } catch (const std::exception&) { rejected = true; }
    assert(rejected);
}
std::unique_ptr<w::Queue> queue(const char* id, w::Precision precision, bool output) {
    return std::make_unique<w::Queue>(w::Format{48000, {"FL", "FR"}, precision}, w::Binding{7, 1}, 11,
        w::Provenance{output ? w::Origin::AdaptedEngine : w::Origin::Synthetic, id});
}
void render(w::Renderer& renderer, w::Queue& q, uint64_t now, uint64_t first, const double* expected) {
    alignas(8) std::array<uint8_t, 32> bytes{};
    assert(renderer.render(now, first, 2, bytes.data(), 2 * q.format().frameBytes()));
    for (size_t i = 0; i < 4; ++i) {
        if (q.format().precision == w::Precision::Float64) {
            double value; std::memcpy(&value, bytes.data() + 8 * i, 8); assert(value == expected[i]);
        } else {
            float value; std::memcpy(&value, bytes.data() + 4 * i, 4); assert(value == static_cast<float>(expected[i]));
        }
    }
}
void run(std::string request, w::Precision inputPrecision, w::Precision recordPrecision) {
    if (inputPrecision == w::Precision::Float32) {
        for (size_t at = 0; (at = request.find("f64-le", at)) != std::string::npos; at += 6)
            request.replace(at, 6, "f32-le");
    }
    auto privateQ = queue("owned-source-private", inputPrecision, false);
    auto returnQ = queue("owned-source-return", inputPrecision, false);
    auto sharedQ = queue("owned-source-shared", inputPrecision, false);
    auto callQ = queue("owned-consumer-call", w::Precision::Float64, true);
    auto recordQ = queue("owned-consumer-record", recordPrecision, true);
    std::vector<m::Port> inputs{{privateQ.get(), "owned-source-private", {"FL", "FR"}},
        {returnQ.get(), "owned-source-return", {"FL", "FR"}}, {sharedQ.get(), "owned-source-shared", {"FL", "FR"}}};
    std::vector<m::Port> outputs{{callQ.get(), "owned-consumer-call", {"FL", "FR"}},
        {recordQ.get(), "owned-consumer-record", {"FL", "FR"}}};
    auto host = m::HostLink::create(request, m::Timeline::injected(11), inputs, outputs);
    auto bad = inputs; bad[2].observedId = "unobserved-process";
    rejects([&] { m::HostLink::create(request, m::Timeline::injected(11), bad, outputs); });
    bad = inputs; std::swap(bad[0], bad[2]);
    rejects([&] { m::HostLink::create(request, m::Timeline::injected(11), bad, outputs); });
    rejects([] { m::Timeline::physical(11); });
    w::Renderer call(*callQ), record(*recordQ);
    const double precise[3][4]{{0.875, -0.875, 0.5, -0.5}, {0.5, -0.5, -0.25, 0.25},
        {0.25 + 0x1p-45, -0.25 - 0x1p-45, 0.125 + 0x1p-46, -0.125 - 0x1p-46}};
    float floats[3][4]{}; double expectedShared[4]{}, expectedRecord[4]{};
    for (size_t s = 0; s < 3; ++s) for (size_t i = 0; i < 4; ++i) floats[s][i] = static_cast<float>(precise[s][i]);
    for (uint64_t seq = 0; seq < 512; ++seq) {
        const uint64_t now = 100 + seq * 50000;
        const w::Stamp stamp{{7, 1}, 11, seq, seq * 2, now, now + 1000000};
        realtime = true;
        if (seq == 256) sharedQ->fail(w::Fault::Shape);
        for (size_t s = 0; s < 3; ++s) {
            if (s == 2 && seq >= 256) continue;
            const void* buffers[]{inputPrecision == w::Precision::Float64 ? static_cast<const void*>(precise[s]) : floats[s]};
            const uint32_t sizes[]{2 * inputs[s].queue->format().frameBytes()};
            assert(inputs[s].queue->push(now, stamp, 2, buffers, sizes, 1, false));
        }
        assert(host->step(now, {7, 1, 11, seq, seq * 2, now, now + 1000000}, 2));
        for (size_t i = 0; i < 4; ++i) {
            expectedShared[i] = seq < 256 ? (inputPrecision == w::Precision::Float64 ? precise[2][i] : floats[2][i]) : 0;
            expectedRecord[i] = 0.25 * expectedShared[i] + 0.125 * precise[1][i];
        }
        // One deliberately stalled reader fills only its own eight-slot queue.
        if (seq < 32) render(call, *callQ, now, seq * 2, expectedShared);
        render(record, *recordQ, now, seq * 2, expectedRecord);
        if (seq >= 40) assert(callQ->fault() == w::Fault::Overflow);
        realtime = false;
    }
    realtime = true;
    if (inputPrecision == w::Precision::Float64) {
        assert(!host->step(30000000, {7, 2, 11, 512, 1024, 30000000, 31000000}, 2));
    } else {
        host->cancel();
        assert(!host->step(30000000, {7, 1, 11, 512, 1024, 30000000, 31000000}, 2));
    }
    assert(recordQ->fault() == w::Fault::Revoked);
    assert(!privateQ->writable() && !returnQ->writable() && !sharedQ->writable());
    realtime = false;
    assert(heapOperations == 0);
}
}
int main(int argc, char** argv) {
    assert(argc == 2); std::ifstream file(argv[1]);
    const std::string request((std::istreambuf_iterator<char>(file)), {});
    assert(!request.empty());
    for (auto source : {w::Precision::Float32, w::Precision::Float64})
        for (auto sink : {w::Precision::Float32, w::Precision::Float64}) run(request, source, sink);
    std::puts("PASS actual macOS host transport + static Rust host: 2048 SPSC/mix/DSP/consumer blocks, Float32/64 precision, stalled consumer isolation, permanent source-loss silence, metadata mismatch, stale/cancel; zero heap operations. Synthetic common clock only; no Apple native APIs.");
}
