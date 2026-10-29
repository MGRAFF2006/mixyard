// SPDX-License-Identifier: MIT
// Linked production Rust host and C++ queues. Windows additionally uses injected
// SDK COM endpoints/driver operations. No device API is called by this program.
#include "bridge.hpp"
#include "../worker_render.hpp"
#ifdef _WIN32
#include "sdk_endpoints.hpp"
#include "../virtual-input/capture/delivery.hpp"
#endif
#include <cstring>
#include <iostream>
#include <stdexcept>

#ifdef MIXYARD_ALLOCATION_AUDIT
namespace {
bool audit_heap = false;
size_t heap_operations = 0;
}
extern "C" {
void* __real_malloc(size_t);
void* __real_calloc(size_t, size_t);
void* __real_realloc(void*, size_t);
void __real_free(void*);
void* __wrap_malloc(size_t count) { if (audit_heap) ++heap_operations; return __real_malloc(count); }
void* __wrap_calloc(size_t count, size_t width) { if (audit_heap) ++heap_operations; return __real_calloc(count, width); }
void* __wrap_realloc(void* pointer, size_t count) { if (audit_heap) ++heap_operations; return __real_realloc(pointer, count); }
void __wrap_free(void* pointer) { if (audit_heap) ++heap_operations; __real_free(pointer); }
}
#endif

namespace {
using namespace mixyard_windows_host;
void require(bool condition, const char* message) { if (!condition) throw std::runtime_error(message); }
constexpr myw_format f64{48000, 3, 2, 64, 64, 1};
constexpr myw_format s24{48000, 3, 2, 24, 24, 0};
constexpr myw_format f32{48000, 3, 2, 32, 32, 1};
Endpoint endpoint(const char* id, myw_format format, bool output) {
    Endpoint e;
    e.observed_id = std::string("injected:") + id;
    e.endpoint_id = std::string("private-sdk:") + id;
    e.provenance = "owned SDK-injected endpoint; no physical MMDevice or app claim";
    e.consent = "this disposable synthetic test session only";
    e.exclusions = "all human/device/default/service graphs; producer recapture";
    e.format = format; e.injected = true; e.owned_output = output;
    e.allow_float32_destination = output && format.container_bits == 32;
    return e;
}
std::vector<Endpoint> sources() { return {endpoint("a-source", f64, false), endpoint("b-source", s24, false)}; }
std::vector<Endpoint> outputs() { return {endpoint("a-wide", f64, true), endpoint("b-narrow", f32, true), endpoint("c-isolated", f64, true)}; }
Timeline timeline(uint64_t generation = 17) {
    return {ClockKind::injected_common_timeline, generation, 7, 91, "explicit injected SDK timeline; physical clocks unqualified"};
}
bool admit(void*, const Endpoint& e, int direction, const Timeline& t) {
    return e.injected && t.kind == ClockKind::injected_common_timeline &&
        e.endpoint_id == "private-sdk:" + e.observed_id.substr(9) &&
        e.consent == "this disposable synthetic test session only" &&
        ((direction == MYW_RENDER) == e.owned_output);
}
std::string request(uint64_t generation) {
    // Deliberately scramble Config order: host plan arrays are canonical ID order.
    const std::string config = R"({"schemaVersion":2,"revision":7,"sources":[
      {"id":"b-source","selector":{"kind":"mic","deviceUid":"private-sdk:b-source"}},
      {"id":"a-source","selector":{"kind":"mic","deviceUid":"private-sdk:a-source"}}],
      "mixes":[{"id":"mix-b","name":"B","channels":2},{"id":"mix-a","name":"A","channels":2}],
      "sends":[{"source":"a-source","mix":"mix-a","gain":0.5},{"source":"b-source","mix":"mix-a","gain":0.25},
               {"source":"b-source","mix":"mix-b","gain":1}],
      "outputs":[{"id":"c-isolated","mix":"mix-b","target":{"kind":"device","deviceUid":"private-sdk:c-isolated"},"gain":1,"allowMicMonitor":true},
       {"id":"b-narrow","mix":"mix-b","target":{"kind":"device","deviceUid":"private-sdk:b-narrow"},"gain":0.5,"allowMicMonitor":true},
       {"id":"a-wide","mix":"mix-a","target":{"kind":"device","deviceUid":"private-sdk:a-wide"},"gain":1,"allowMicMonitor":true}]})";
    std::string inventory = R"({"schemaVersion":2,"revision":7,"platform":"windows","evidence":"simulated","sources":[)";
    const auto input = sources(); const auto output = outputs();
    for (size_t i = 0; i < input.size(); ++i) {
        if (i) inventory += ',';
        inventory += "{\"observedId\":\"" + input[i].observed_id + "\",\"selector\":{\"kind\":\"mic\",\"deviceUid\":\"" + input[i].endpoint_id +
            "\"},\"origin\":\"external\",\"format\":{\"channels\":2,\"sampleRateHz\":48000}}";
    }
    inventory += "],\"outputs\":[";
    for (size_t i = 0; i < output.size(); ++i) {
        if (i) inventory += ',';
        inventory += "{\"observedId\":\"" + output[i].observed_id + "\",\"target\":{\"kind\":\"device\",\"deviceUid\":\"" + output[i].endpoint_id +
            "\"},\"format\":{\"channels\":2,\"sampleRateHz\":48000}}";
    }
    inventory += "],\"limits\":{}}";
    std::string result = "{\"config\":" + config + ",\"inventory\":" + inventory + ",\"generation\":" + std::to_string(generation) +
        ",\"clockId\":91,\"maxFrames\":256,\"limits\":{\"maxSources\":2,\"maxMixes\":2,\"maxSends\":3,\"maxOutputs\":3,\"maxBufferSamples\":8192,\"maxTotalBytes\":1048576,\"maxDspWorkUnitsPerBlock\":1000000},\"sources\":[";
    for (size_t i = 0; i < input.size(); ++i) { if (i) result += ','; result += source_spec(input[i]); }
    result += "],\"outputs\":[";
    for (size_t i = 0; i < output.size(); ++i) { if (i) result += ','; result += output_spec(output[i]); }
    return result + "]}";
}
struct Session {
    mx_host* host = nullptr;
    std::unique_ptr<Bridge> bridge;
    Timeline time;
    explicit Session(uint64_t generation = 17) : time(timeline(generation)) {
        const auto json = request(generation);
        std::array<char, 2048> error{};
        const auto result = mx_host_create(reinterpret_cast<const uint8_t*>(json.data()), json.size(), &host, error.data(), error.size());
        if (result != 0) throw std::runtime_error(std::string("actual Rust host create: ") + error.data());
        if (Bridge::create(host, time, sources(), outputs(), {nullptr, admit}, bridge) != Result::ok) {
            mx_host_destroy(host); host = nullptr; throw std::runtime_error("actual host info/Windows bridge binding failed");
        }
    }
    ~Session() { bridge.reset(); mx_host_destroy(host); }
};
mx_stamp engine(const Timeline& t, uint64_t sequence, uint64_t first_frame) {
    // Explicit test timeline, no multiplication/offset from QPC or nominal rate.
    return {t.generation, t.revision, t.clock_id, sequence, first_frame, 1000000 + sequence * 1000, 2000000 + sequence * 1000};
}
const std::array<double, 4> wide{{0.5 + 0x1p-40, -0.25 - 0x1p-41, 0.125 + 0x1p-42, -0.5 - 0x1p-40}};
const std::array<uint8_t, 12> integer{{0x00, 0x00, 0x20, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x60, 0x00, 0x00, 0xf0}};
const std::array<double, 4> integer_values{{0.25, -0.5, 0.75, -0.125}};
std::array<MappedTime, 2> push(Session& s, uint64_t sequence, uint64_t frame, bool second = true) {
    const auto stamp = engine(s.time, sequence, frame);
    std::array<MappedTime, 2> mapped{};
    for (size_t i = 0; i < mapped.size(); ++i) {
        mapped[i] = {stamp, 200000 + frame, 777777777 + sequence * 13, 0};
        if (i && !second) continue;
        myw_stamp native{{s.time.generation, s.time.revision}, s.time.clock_id, sequence, frame,
                         mapped[i].device_frame, mapped[i].qpc_100ns, 0, 0};
        const auto result = i ? myw_port_push(s.bridge->source(i), s24, native, integer.data(), 2, integer.size()) :
            myw_port_push(s.bridge->source(i), f64, native, wide.data(), 2, sizeof(wide));
        require(result == MYW_OK, "production native capture queue admission");
    }
    return mapped;
}
void process(Session& s, uint64_t sequence, uint64_t frame, const std::array<MappedTime, 2>& times) {
    const auto stamp = engine(s.time, sequence, frame);
    require(s.bridge->process(stamp.captured_ns, stamp, 2, times.data(), times.size()) == Result::ok,
        "real Rust canonical mixer process");
}
template<class T> void consume(Session& s, size_t index, const std::array<T, 4>& expected) {
    mixyard_worker::RenderCursor cursor;
    std::array<uint8_t, sizeof(expected)> bytes{};
    uint64_t missing = 0;
    const myw_binding binding{s.time.generation, s.time.revision};
    // Exercise partial native requests, including one staged packet across calls.
    require(cursor.fill(s.bridge->output(index), binding, sizeof(T) * 2, bytes.data(), 1, missing) == MYW_OK, "first native render frame");
    require(cursor.fill(s.bridge->output(index), binding, sizeof(T) * 2, bytes.data() + sizeof(T) * 2, 1, missing) == MYW_OK, "second native render frame");
    require(!missing && std::memcmp(bytes.data(), expected.data(), sizeof(expected)) == 0, "consumer byte oracle; exact ordered samples/precision/gains");
}
void oracle(Session& s, bool b = true) {
    std::array<double, 4> a{}, c{}; std::array<float, 4> narrow{};
    for (size_t i = 0; i < a.size(); ++i) {
        const auto value = b ? integer_values[i] : 0.0;
        a[i] = wide[i] * 0.5 + value * 0.25; c[i] = value; narrow[i] = static_cast<float>(value * 0.5);
    }
    consume(s, 0, a); consume(s, 1, narrow); consume(s, 2, c);
}
void portable_checks() {
    {
        Session s;
        auto mapped = push(s, 0, 0); process(s, 0, 0, mapped); oracle(s);
        // Endpoint loss removes only B; historical PCM must never be replayed.
        myw_port_revoke(s.bridge->source(1));
        mapped = push(s, 1, 2, false); process(s, 1, 2, mapped); oracle(s, false);
        // A lost destination cannot poison the remaining independently queued mix.
        myw_port_revoke(s.bridge->output(1));
        mapped = push(s, 2, 4, false); process(s, 2, 4, mapped);
        std::array<double, 4> a{}; for (size_t i = 0; i < a.size(); ++i) a[i] = wide[i] * 0.5;
        consume(s, 0, a); consume(s, 2, std::array<double, 4>{});
        s.bridge->cancel();
        const auto stamp = engine(s.time, 3, 6);
        require(s.bridge->process(stamp.captured_ns, stamp, 2, mapped.data(), mapped.size()) == Result::cancelled, "cancel acknowledged on graph owner");
        std::array<uint8_t, 4096> bytes{}; myw_block block{};
        bytes.fill(0xff);
        require(myw_port_pop(s.bridge->output(0), {17, 7}, bytes.data(), bytes.size(), &block) != MYW_OK, "revoke blocks old output");
        for (auto byte : bytes) require(!byte, "no stale data on revoked output");
    }
    {
        Session fresh(18); auto mapped = push(fresh, 0, 0); process(fresh, 0, 0, mapped); oracle(fresh);
        const auto old = engine(timeline(17), 1, 2);
        require(fresh.bridge->process(old.captured_ns, old, 2, mapped.data(), mapped.size()) == Result::invalid, "old generation cannot drive restarted session");
    }
    {
        Session s; auto mapped = push(s, 0, 0); ++mapped[0].qpc_100ns;
        const auto stamp = engine(s.time, 0, 0);
        require(s.bridge->process(stamp.captured_ns, stamp, 2, mapped.data(), mapped.size()) == Result::source_time, "QPC provenance mismatch fails closed");
    }
    {
        Session s; std::unique_ptr<Bridge> rejected;
        auto physical = s.time; physical.kind = ClockKind::physical_unqualified;
        require(Bridge::create(s.host, physical, sources(), outputs(), {nullptr, admit}, rejected) == Result::clock, "same nominal rate cannot qualify physical clocks");
        auto wrong = sources(); wrong[0].format.valid_bits = 32;
        require(Bridge::create(s.host, s.time, wrong, outputs(), {nullptr, admit}, rejected) == Result::binding, "valid-bit mismatch rejects before source conversion");
        wrong = sources(); wrong[0].observed_id = "injected:wrong";
        require(Bridge::create(s.host, s.time, wrong, outputs(), {nullptr, admit}, rejected) == Result::binding, "observed identity cannot be substituted");
        auto denied = outputs(); denied[1].allow_float32_destination = false;
        require(Bridge::create(s.host, s.time, sources(), denied, {nullptr, admit}, rejected) == Result::binding, "narrow sink requires explicit destination consent");
        wrong = sources(); wrong[0].endpoint_id = outputs()[0].endpoint_id;
        require(Bridge::create(s.host, s.time, wrong, outputs(), {nullptr, admit}, rejected) == Result::binding, "output recapture excluded");
    }
}
void sustained_link_checks() {
    Session s;
#ifdef MIXYARD_ALLOCATION_AUDIT
    heap_operations = 0; audit_heap = true;
#endif
    for (uint64_t sequence = 0; sequence < 512; ++sequence) {
        auto mapped = push(s, sequence, sequence * 2);
        process(s, sequence, sequence * 2, mapped);
        oracle(s);
    }
    // Revoke/failure paths must also avoid heap operations on the graph owner.
    s.bridge->cancel();
    const auto stamp = engine(s.time, 512, 1024);
    std::array<MappedTime, 2> times{};
    const auto result = s.bridge->process(stamp.captured_ns, stamp, 2, times.data(), times.size());
#ifdef MIXYARD_ALLOCATION_AUDIT
    audit_heap = false;
    require(!heap_operations, "full native queues/Rust host/render path performed heap operation");
#endif
    require(result == Result::cancelled, "sustained path cancellation");
}
#ifdef _WIN32
// These objects implement the real Windows SDK vtables. They never activate a
// device or call an OS audio API. The production seam sees the same interfaces
// and error codes as a separately authorized initialized WASAPI worker.
class Capture final : public IAudioCaptureClient {
    std::array<uint8_t, 64> bytes_{};
public:
    uint64_t device = 200000, qpc = 777777777;
    bool pending = true, lost = false;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void** out) override { if (out) *out = nullptr; return E_NOINTERFACE; }
    ULONG STDMETHODCALLTYPE AddRef() override { return 1; }
    ULONG STDMETHODCALLTYPE Release() override { return 1; }
    Capture(const void* bytes, size_t count) { require(count <= bytes_.size(), "injected capture bound"); std::memcpy(bytes_.data(), bytes, count); }
    HRESULT STDMETHODCALLTYPE GetNextPacketSize(UINT32* frames) override {
        if (lost) return AUDCLNT_E_DEVICE_INVALIDATED;
        *frames = pending ? 2 : 0; return S_OK;
    }
    HRESULT STDMETHODCALLTYPE GetBuffer(BYTE** data, UINT32* frames, DWORD* flags, UINT64* position, UINT64* timestamp) override {
        if (lost) return AUDCLNT_E_DEVICE_INVALIDATED;
        if (!pending) return AUDCLNT_S_BUFFER_EMPTY;
        *data = bytes_.data(); *frames = 2; *flags = 0; *position = device; *timestamp = qpc; return S_OK;
    }
    HRESULT STDMETHODCALLTYPE ReleaseBuffer(UINT32 frames) override {
        if (frames != 2 || !pending) return E_INVALIDARG;
        pending = false; return S_OK;
    }
};
class Render final : public IAudioRenderClient {
public:
    std::array<uint8_t, 64> bytes{};
    size_t width;
    bool lost = false;
    uint32_t released = 0, flags = 0;
    explicit Render(size_t frame_bytes) : width(frame_bytes) {}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void** out) override { if (out) *out = nullptr; return E_NOINTERFACE; }
    ULONG STDMETHODCALLTYPE AddRef() override { return 1; }
    ULONG STDMETHODCALLTYPE Release() override { return 1; }
    HRESULT STDMETHODCALLTYPE GetBuffer(UINT32 frames, BYTE** data) override {
        if (lost) return AUDCLNT_E_DEVICE_INVALIDATED;
        if (!frames || frames * width > bytes.size()) return E_INVALIDARG;
        bytes.fill(0xcc); *data = bytes.data(); return S_OK;
    }
    HRESULT STDMETHODCALLTYPE ReleaseBuffer(UINT32 frames, DWORD native_flags) override {
        released = frames; flags = native_flags;
        if (flags & AUDCLNT_BUFFERFLAGS_SILENT) bytes.fill(0);
        return S_OK;
    }
};
mixyard_input::Transport driver{};
unsigned driver_closed = 0, driver_submitted = 0;
HANDLE driver_open() {
    require(driver.begin(1, 123) == mixyard_input::Result::accepted, "injected driver admission");
    return reinterpret_cast<HANDLE>(static_cast<uintptr_t>(1));
}
BOOL driver_close(HANDLE) { ++driver_closed; (void)driver.cleanup(1); return TRUE; }
BOOL driver_ioctl(HANDLE, DWORD code, LPVOID input, DWORD input_bytes,
                   LPVOID output, DWORD output_bytes, LPDWORD returned, LPOVERLAPPED) {
    using namespace mixyard_input;
    if (code == ioctl_capture_status && !input && !input_bytes && output_bytes == sizeof(CaptureStatus)) {
        CaptureStatus status{};
        status.version = control_version; status.bytes = sizeof(status);
        status.generation = driver.generation; status.producer_epoch = driver.epoch;
        status.next_sequence = driver.next_sequence; status.next_frame = driver.next_frame;
        status.overwritten_packets = driver.overwritten_packets;
        std::memcpy(output, &status, sizeof(status)); *returned = sizeof(status); return TRUE;
    }
    if (code == ioctl_submit && output_bytes == sizeof(SubmitReply)) {
        ++driver_submitted;
        SubmitReply reply{};
        reply.version = control_version; reply.bytes = sizeof(reply);
        reply.result = static_cast<uint32_t>(driver.submit(1, input, input_bytes));
        std::memcpy(output, &reply, sizeof(reply)); *returned = sizeof(reply); return TRUE;
    }
    return FALSE;
}
void sdk_checks() {
    {
        Session s;
        Capture a(wide.data(), sizeof(wide)), b(integer.data(), integer.size());
        CaptureEndpoint capture_a(s.bridge->source(0), &a), capture_b(s.bridge->source(1), &b);
        require(capture_a.service() == MYW_OK && capture_b.service() == MYW_OK, "production SDK capture clients -> native queues");
        const auto stamp = engine(s.time, 0, 0);
        std::array<MappedTime, 2> mapped{{{stamp, a.device, a.qpc, 0}, {stamp, b.device, b.qpc, 0}}};
        process(s, 0, 0, mapped);
        Render wide_sink(16), narrow_sink(8), isolated_sink(16);
        RenderEndpoint render_a(s.bridge->output(0), &wide_sink), render_b(s.bridge->output(1), &narrow_sink), render_c(s.bridge->output(2), &isolated_sink);
        require(render_a.service(2) == MYW_OK && render_b.service(2) == MYW_OK && render_c.service(2) == MYW_OK, "actual linked Rust output -> SDK native render consumers");
        std::array<double, 4> expected{}; std::array<float, 4> narrow{};
        for (size_t i = 0; i < expected.size(); ++i) { expected[i] = wide[i] * 0.5 + integer_values[i] * 0.25; narrow[i] = static_cast<float>(integer_values[i] * 0.5); }
        require(wide_sink.released == 2 && !wide_sink.flags && std::memcmp(wide_sink.bytes.data(), expected.data(), sizeof(expected)) == 0,
            "SDK render consumer Float64 byte oracle retains sub-Float32 details");
        require(std::memcmp(narrow_sink.bytes.data(), narrow.data(), sizeof(narrow)) == 0 &&
                std::memcmp(isolated_sink.bytes.data(), integer_values.data(), sizeof(integer_values)) == 0,
            "SDK destination-only Float32 and independent Float64 oracle");
        // Actual SDK endpoint-loss error paths and independent surviving output.
        a.lost = true;
        require(capture_a.service() == MYW_NATIVE, "SDK capture endpoint loss revokes its source");
        b.pending = true; b.device += 2; b.qpc += 13;
        require(capture_b.service() == MYW_OK, "unrelated capture survives endpoint loss");
        const auto next = engine(s.time, 1, 2);
        mapped[1] = {next, b.device, b.qpc, 0};
        process(s, 1, 2, mapped);
        narrow_sink.lost = true;
        require(render_b.service(2) == MYW_NATIVE, "SDK output loss closes only failed destination");
        require(render_a.service(2) == MYW_OK && render_c.service(2) == MYW_OK, "sibling SDK output still consumes its mix");
        for (size_t i = 0; i < expected.size(); ++i) expected[i] = integer_values[i] * 0.25;
        require(std::memcmp(wide_sink.bytes.data(), expected.data(), sizeof(expected)) == 0, "lost source is fresh silence, never retained PCM");
        s.bridge->cancel();
        require(render_a.service(1) != MYW_OK, "cancelled SDK consumer cannot admit another frame");
    }
    {
        driver.initialize();
        Session s;
        IngressConsumer ingress(s.bridge->output(1), {driver_open, driver_ioctl, driver_close});
        require(ingress.open(), "explicit existing producer boundary with injected driver only");
        auto mapped = push(s, 0, 0); process(s, 0, 0, mapped);
        require(ingress.service() == MYW_OK && driver_submitted == 1, "Rust mix -> independent native queue -> production StreamProducer serialization");
        mixyard_input::Delivery consumer;
        std::array<uint32_t, 4> observed{}, expected{};
        for (size_t i = 0; i < 2; ++i) consumer.frame(driver, observed.data() + i * 2);
        std::array<float, 4> narrow{};
        for (size_t i = 0; i < narrow.size(); ++i) narrow[i] = static_cast<float>(integer_values[i] * 0.5);
        std::memcpy(expected.data(), narrow.data(), sizeof(narrow));
        require(observed == expected && consumer.copied_frames == 2 && !consumer.silent_frames,
            "existing driver Delivery consumer byte oracle; not selectable mic proof");
        ++driver.epoch;
        mapped = push(s, 1, 2); process(s, 1, 2, mapped);
        require(ingress.service() != MYW_OK && driver_submitted == 1 && driver_closed == 1,
            "driver restart closes before submitting old-generation PCM");
        std::array<double, 4> a{}; for (size_t i = 0; i < a.size(); ++i) a[i] = wide[i] * 0.5 + integer_values[i] * 0.25;
        // First queued block still belongs to the independent Float64 render sink.
        consume(s, 0, a);
    }
}
#endif
} // namespace

int main() {
    try {
        portable_checks();
        sustained_link_checks();
#ifdef _WIN32
        sdk_checks();
        std::cout << "PASS SDK-injected capture/render endpoints and production driver Delivery byte oracle; no hardware APIs\n";
#endif
        std::cout << "PASS production Windows queues -> actual Rust Processor64/planner -> independent native consumers; physical/native availability false\n";
#ifdef MIXYARD_ALLOCATION_AUDIT
        std::cout << "PASS 512 linked native blocks plus cancellation; zero malloc/calloc/realloc/free\n";
#endif
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
