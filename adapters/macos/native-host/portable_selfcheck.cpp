// SPDX-License-Identifier: MIT
#include "host_link.hpp"
#include "owned_queue.hpp"
#include <cassert>
#include <cstdio>
#include <fstream>
#include <iterator>

static bool realtime = false;
static unsigned heapOperations = 0;
static unsigned hostRevocations = 0;
extern "C" void __real_mx_host_revoke(mx_host*);
extern "C" void __wrap_mx_host_revoke(mx_host* host) { ++hostRevocations; __real_mx_host_revoke(host); }
extern "C" void* __real_malloc(size_t);
extern "C" void* __real_calloc(size_t, size_t);
extern "C" void* __real_realloc(void*, size_t);
extern "C" void __real_free(void*);
extern "C" void* __wrap_malloc(size_t n) { if (realtime) ++heapOperations; return __real_malloc(n); }
extern "C" void* __wrap_calloc(size_t n, size_t s) { if (realtime) ++heapOperations; return __real_calloc(n, s); }
extern "C" void* __wrap_realloc(void* p, size_t n) { if (realtime) ++heapOperations; return __real_realloc(p, n); }
extern "C" void __wrap_free(void* p) { if (realtime) ++heapOperations; __real_free(p); }
namespace mixyard::macos::detail {
void queueScrubbed(const void* storage, size_t bytes) noexcept {
    assert(!realtime);
    const auto* data = static_cast<const unsigned char*>(storage);
    for (size_t i = 0; i < bytes; ++i) assert(data[i] == 0);
}
}
namespace m = mixyard::macos;
namespace w = mixyard::worker;
namespace {
template<class F> void rejects(F f) {
    bool rejected = false;
    try { f(); } catch (const std::exception&) { rejected = true; }
    assert(rejected);
}
m::OwnedQueue queue(const char* id, w::Precision precision, bool output, uint64_t generation = 7) {
    return m::ownQueue(w::Format{48000, {"FL", "FR"}, precision}, w::Binding{generation, 1}, 11,
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
        for (size_t s = 0; s < 3; ++s) {
            const void* buffers[]{inputPrecision == w::Precision::Float64 ? static_cast<const void*>(precise[s]) : floats[s]};
            const uint32_t sizes[]{2 * inputs[s].queue->format().frameBytes()};
            assert(inputs[s].queue->push(now, stamp, 2, buffers, sizes, 1, false));
        }
        assert(host->step(now, {7, 1, 11, seq, seq * 2, now, now + 1000000}, 2) == m::StepResult::Processed);
        for (size_t i = 0; i < 4; ++i) {
            expectedShared[i] = inputPrecision == w::Precision::Float64 ? precise[2][i] : floats[2][i];
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
        const w::Stamp fresh{{7, 1}, 11, 512, 1024, 30000000, 31000000};
        for (size_t s = 0; s < inputs.size(); ++s) {
            const void* buffers[]{precise[s]}; const uint32_t sizes[]{sizeof(precise[s])};
            assert(inputs[s].queue->push(30000000, fresh, 2, buffers, sizes, 1, false));
        }
        assert(host->step(30000000, {7, 2, 11, 512, 1024, 30000000, 31000000}, 2) == m::StepResult::HostFailure);
    } else {
        host->cancel();
        assert(host->step(30000000, {7, 1, 11, 512, 1024, 30000000, 31000000}, 2) == m::StepResult::Cancelled);
    }
    assert(recordQ->fault() == w::Fault::Revoked);
    assert(!privateQ->writable() && !returnQ->writable() && !sharedQ->writable());
    realtime = false;
    assert(heapOperations == 0);
}
void emptySourceIsTerminal(const std::string& request) {
    auto privateQ = queue("owned-source-private", w::Precision::Float64, false);
    auto returnQ = queue("owned-source-return", w::Precision::Float64, false);
    auto sharedQ = queue("owned-source-shared", w::Precision::Float64, false);
    auto callQ = queue("owned-consumer-call", w::Precision::Float64, true);
    auto recordQ = queue("owned-consumer-record", w::Precision::Float64, true);
    std::vector<m::Port> inputs{{privateQ.get(),"owned-source-private",{"FL","FR"}},
        {returnQ.get(),"owned-source-return",{"FL","FR"}},{sharedQ.get(),"owned-source-shared",{"FL","FR"}}};
    auto host=m::HostLink::create(request,m::Timeline::injected(11),inputs,
        {{callQ.get(),"owned-consumer-call",{"FL","FR"}},{recordQ.get(),"owned-consumer-record",{"FL","FR"}}});
    w::Renderer call(*callQ), record(*recordQ);
    const double pcm[]{0.5, -0.5, 0.25, -0.25};
    const void* buffers[]{pcm}; const uint32_t sizes[]{sizeof(pcm)};
    const w::Stamp stamp{{7, 1}, 11, 0, 0, 100, 1000100};
    const auto revocations = hostRevocations;
    realtime = true;
    assert(privateQ->push(100, stamp, 2, buffers, sizes, 1, false));
    assert(returnQ->push(100, stamp, 2, buffers, sizes, 1, false));
    assert(host->step(100, {7, 1, 11, 0, 0, 100, 1000100}, 2) == m::StepResult::SourceLoss);
    assert(hostRevocations == revocations + 1);
    assert(sharedQ->fault() == w::Fault::Revoked && privateQ->fault() == w::Fault::Revoked && returnQ->fault() == w::Fault::Revoked);
    assert(callQ->fault() == w::Fault::Revoked && recordQ->fault() == w::Fault::Revoked);
    assert(!sharedQ->push(100, stamp, 2, buffers, sizes, 1, false));
    assert(host->step(100, {7, 1, 11, 0, 0, 100, 1000100}, 2) == m::StepResult::SourceLoss);
    assert(hostRevocations == revocations + 1);
    double zero[4]{1, 1, 1, 1};
    assert(!call.render(100, 0, 2, zero, sizeof(zero)));
    for (const auto value : zero) assert(value == 0);
    realtime = false;
    assert(heapOperations==0);
}

std::string callEffect(std::string request, const char* effect) {
    const auto output = request.find("\"outputDsp\"");
    const auto slot = request.find("null", output);
    assert(output != std::string::npos && slot != std::string::npos);
    request.replace(slot, 4, std::string("{\"name\":\"native-retained-pcm\",\"format\":{\"sampleRateHz\":48000,\"channels\":[\"FL\",\"FR\"]},\"effects\":[{\"name\":\"regression\",\"enabled\":true,\"effect\":") + effect + "}]}");
    return request;
}
void queuedSourceLoss(const std::string& request, bool partial, bool empty) {
    auto privateQ = queue("owned-source-private", w::Precision::Float64, false);
    auto returnQ = queue("owned-source-return", w::Precision::Float64, false);
    auto sharedQ = queue("owned-source-shared", w::Precision::Float64, false);
    auto callQ = queue("owned-consumer-call", w::Precision::Float64, true);
    auto recordQ = queue("owned-consumer-record", w::Precision::Float64, true);
    std::vector<m::Port> inputs{{privateQ.get(), "owned-source-private", {"FL", "FR"}},
        {returnQ.get(), "owned-source-return", {"FL", "FR"}}, {sharedQ.get(), "owned-source-shared", {"FL", "FR"}}};
    auto host = m::HostLink::create(callEffect(request, "{\"kind\":\"delay\",\"frames\":2,\"wet\":1,\"dry\":0,\"feedback\":0}"), m::Timeline::injected(11), inputs,
        {{callQ.get(), "owned-consumer-call", {"FL", "FR"}}, {recordQ.get(), "owned-consumer-record", {"FL", "FR"}}});
    w::Renderer call(*callQ), record(*recordQ);
    const double pcm[]{0.5, -0.5, 0.25, -0.25}, zero[4]{}, recorded[]{0.1875, -0.1875, 0.09375, -0.09375};
    const void* buffers[]{pcm}; const uint32_t sizes[]{sizeof(pcm)};
    realtime = true;
    for (uint64_t seq = 0; seq < 2; ++seq) {
        const uint64_t now = 100 + seq * 50000;
        const w::Stamp s{{7, 1}, 11, seq, seq * 2, now, now + 1000000};
        for (const auto& input : inputs) assert(input.queue->push(now, s, 2, buffers, sizes, 1, false));
        assert(host->step(now, {7, 1, 11, seq, seq * 2, now, now + 1000000}, 2) == m::StepResult::Processed);
        if (!seq) { render(call, *callQ, now, 0, zero); render(record, *recordQ, now, 0, recorded); }
    }
    // Real output DSP now has nonzero old-source PCM in native port storage.
    const auto* pending = callQ->front(50100); assert(pending);
    double first; std::memcpy(&first, pending->pcm.data(), 8); assert(first == 0.5);
    double sample[2]{};
    if (partial) {
        assert(call.render(50100, 2, 1, sample, sizeof(sample)) && sample[0] == 0.5);
        assert(record.render(50100, 2, 1, sample, sizeof(sample)) && sample[0] == 0.1875);
    }
    const w::Stamp next{{7, 1}, 11, 2, 4, 100100, 1100100};
    assert(privateQ->push(100100, next, 2, buffers, sizes, 1, false));
    assert(returnQ->push(100100, next, 2, buffers, sizes, 1, false));
    if (!empty) sharedQ->revoke();
    const auto revocations = hostRevocations;
    assert(host->step(100100, {7, 1, 11, 2, 4, 100100, 1100100}, 2) == m::StepResult::SourceLoss);
    assert(hostRevocations == revocations + 1);
    assert(callQ->fault() == w::Fault::Revoked && recordQ->fault() == w::Fault::Revoked);
    for (const auto& input : inputs) assert(!input.queue->push(100100, next, 2, buffers, sizes, 1, false));
    // The production Renderer must discard both queued and staged old PCM.
    double retained[4]{1, 1, 1, 1};
    const auto frames = partial ? 1u : 2u;
    assert(!call.render(100100, partial ? 3 : 2, frames, retained, frames * 16));
    for (unsigned i = 0; i < frames * 2; ++i) assert(retained[i] == 0);
    std::fill(std::begin(retained), std::end(retained), 1);
    assert(!record.render(100100, partial ? 3 : 2, frames, retained, frames * 16));
    for (unsigned i = 0; i < frames * 2; ++i) assert(retained[i] == 0);
    realtime = false;
    assert(heapOperations == 0);
}

void destinationFailureIsIsolated(const std::string& request) {
    auto privateQ = queue("owned-source-private", w::Precision::Float64, false);
    auto returnQ = queue("owned-source-return", w::Precision::Float64, false);
    auto sharedQ = queue("owned-source-shared", w::Precision::Float64, false);
    auto callQ = queue("owned-consumer-call", w::Precision::Float64, true);
    auto recordQ = queue("owned-consumer-record", w::Precision::Float64, true);
    std::vector<m::Port> inputs{{privateQ.get(), "owned-source-private", {"FL", "FR"}},
        {returnQ.get(), "owned-source-return", {"FL", "FR"}}, {sharedQ.get(), "owned-source-shared", {"FL", "FR"}}};
    auto host = m::HostLink::create(callEffect(request, "{\"kind\":\"gain\",\"gain\":1.7976931348623157e308,\"invertPolarity\":false}"), m::Timeline::injected(11), inputs,
        {{callQ.get(), "owned-consumer-call", {"FL", "FR"}}, {recordQ.get(), "owned-consumer-record", {"FL", "FR"}}});
    w::Renderer call(*callQ), record(*recordQ);
    const double pcm[]{8, -8, 4, -4}, expected[]{3, -3, 1.5, -1.5};
    const void* buffers[]{pcm}; const uint32_t sizes[]{sizeof(pcm)};
    realtime = true;
    for (uint64_t seq = 0; seq < 3; ++seq) {
        const uint64_t now = 100 + seq * 50000;
        const w::Stamp s{{7, 1}, 11, seq, seq * 2, now, now + 1000000};
        for (const auto& input : inputs) assert(input.queue->push(now, s, 2, buffers, sizes, 1, false));
        assert(host->step(now, {7, 1, 11, seq, seq * 2, now, now + 1000000}, 2) == m::StepResult::Processed);
        assert(callQ->fault() == w::Fault::Shape && recordQ->fault() == w::Fault::None);
        for (const auto& input : inputs) assert(input.queue->fault() == w::Fault::None);
        render(record, *recordQ, now, seq * 2, expected);
        double silence[4]{1, 1, 1, 1};
        assert(!call.render(now, seq * 2, 2, silence, sizeof(silence)));
        for (const auto value : silence) assert(value == 0);
    }
    realtime = false;
    assert(heapOperations == 0);
}

void restartUsesFreshGeneration(std::string request) {
    // Previous tests retired their host/Renderer/queue storage synchronously.
    const std::string marker = "\"generation\": 7";
    const auto at = request.find(marker); assert(at != std::string::npos);
    request.replace(at, marker.size(), "\"generation\": 8");
    auto privateQ = queue("owned-source-private", w::Precision::Float64, false, 8);
    auto returnQ = queue("owned-source-return", w::Precision::Float64, false, 8);
    auto sharedQ = queue("owned-source-shared", w::Precision::Float64, false, 8);
    auto callQ = queue("owned-consumer-call", w::Precision::Float64, true, 8);
    auto recordQ = queue("owned-consumer-record", w::Precision::Float64, true, 8);
    std::vector<m::Port> inputs{{privateQ.get(), "owned-source-private", {"FL", "FR"}},
        {returnQ.get(), "owned-source-return", {"FL", "FR"}}, {sharedQ.get(), "owned-source-shared", {"FL", "FR"}}};
    auto host = m::HostLink::create(request, m::Timeline::injected(11), inputs,
        {{callQ.get(), "owned-consumer-call", {"FL", "FR"}}, {recordQ.get(), "owned-consumer-record", {"FL", "FR"}}});
    const double pcm[]{0.5, -0.5, 0.25, -0.25}; const uint32_t sizes[]{sizeof(pcm)};
    w::Renderer call(*callQ);
    const double silence[4]{};
    realtime = true;
    for (uint64_t seq = 0; seq < 2; ++seq) {
        const uint64_t now = 100 + seq * 50000;
        const w::Stamp s{{8, 1}, 11, seq, seq * 2, now, now + 1000000};
        const void* fresh[]{seq ? pcm : silence};
        for (const auto& input : inputs) assert(input.queue->push(now, s, 2, fresh, sizes, 1, false));
        assert(host->step(now, {8, 1, 11, seq, seq * 2, now, now + 1000000}, 2) == m::StepResult::Processed);
        render(call, *callQ, now, seq * 2, seq ? pcm : silence);
    }
    realtime = false;
    assert(heapOperations == 0);
}

}
int main(int argc, char** argv) {
    assert(argc == 2); std::ifstream file(argv[1]);
    const std::string request((std::istreambuf_iterator<char>(file)), {});
    assert(!request.empty());
    for (bool partial : {false, true}) for (bool empty : {false, true}) queuedSourceLoss(request, partial, empty);
    emptySourceIsTerminal(request);
    destinationFailureIsIsolated(request);
    restartUsesFreshGeneration(request);
    for (auto source : {w::Precision::Float32, w::Precision::Float64})
        for (auto sink : {w::Precision::Float32, w::Precision::Float64}) run(request, source, sink);
    std::puts("PASS actual macOS host transport + static Rust host: 2048 SPSC/mix/DSP/consumer blocks, Float32/64 precision, stalled consumer isolation, typed whole-generation source-loss cancellation with queued/partial DSP PCM discard, metadata mismatch, stale/cancel; zero heap operations. Synthetic common clock only; no Apple native APIs.");
}
