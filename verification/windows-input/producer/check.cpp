// SPDX-License-Identifier: MIT
// Injected OS operations ONLY, linked into the actual executable's --selfcheck.
#include "../../../adapters/windows/virtual-input/producer/harness.hpp"
#include <cstdio>
#include <thread>

namespace {
using namespace mixyard_producer;
constexpr wchar_t uuid[] = L"76b96d45-cb1c-4d5e-b11b-88831866e401";
constexpr wchar_t actual[] = L"SWD\\decorated-by-injected-operation\\actual-id";
HSWDEVICE software_handle() { return reinterpret_cast<HSWDEVICE>(uintptr_t{17}); }
HANDLE file_handle() { return reinterpret_cast<HANDLE>(uintptr_t{23}); }
struct SoftwareFixture {
    HRESULT create_result = S_OK, callback_result = S_OK;
    bool early = false, null_id = false, null_handle = false, wrong_callback_handle = false;
    unsigned creates = 0, closes = 0;
    SW_DEVICE_CREATE_CALLBACK callback = nullptr;
    void* context = nullptr;
    std::thread worker;
    void complete() {
        callback(wrong_callback_handle ? nullptr : software_handle(), callback_result,
                 context, null_id ? nullptr : actual);
    }
} *software = nullptr;
HRESULT WINAPI create(PCWSTR enumerator, PCWSTR parent, const SW_DEVICE_CREATE_INFO* info,
                      ULONG count, const DEVPROPERTY* properties, SW_DEVICE_CREATE_CALLBACK callback,
                      PVOID context, PHSWDEVICE handle) {
    auto& s = *software;
    ++s.creates;
    require(std::wstring(enumerator) == L"MixyardCapture" && std::wstring(parent) == L"HTREE\\ROOT\\0", "creation identity");
    require(info->cbSize == sizeof(*info) && std::wstring(info->pszInstanceId) == uuid, "explicit UUID");
    require(std::wstring(info->pszzHardwareIds) == L"SWD\\MixyardCapture" &&
        info->pszzHardwareIds[std::wcslen(info->pszzHardwareIds) + 1] == 0, "matching MULTI_SZ");
    require(info->CapabilityFlags == (SWDeviceCapabilitiesRemovable | SWDeviceCapabilitiesDriverRequired) &&
        !info->pszzCompatibleIds && !info->pContainerId && !info->pSecurityDescriptor && !count && !properties,
        "minimal create arguments");
    s.callback = callback; s.context = context;
    *handle = s.null_handle ? nullptr : software_handle();
    if (s.early) s.complete(); // Before the caller receives its output handle.
    return s.create_result;
}
void WINAPI close_software(HSWDEVICE handle) {
    require(handle == software_handle(), "software close identity");
    ++software->closes;
    if (software->worker.joinable()) software->worker.join(); // Injected close/drain contract.
}
template<class F> void fails(F operation) {
    bool failed = false;
    try { operation(); } catch (const std::runtime_error&) { failed = true; }
    require(failed, "expected rejection");
}
void lifecycle_checks() {
    require(valid_uuid(uuid) && !valid_uuid(L"00000000-0000-0000-0000-000000000000") &&
        !valid_uuid(L"76B96D45-cb1c-4d5e-b11b-88831866e401") && !valid_uuid(L"name"), "UUID validation");
    for (bool early : {false, true}) {
        SoftwareFixture fixture; software = &fixture; fixture.early = early;
        {
            Provision p({create, close_software});
            require(p.start(uuid, L"HTREE\\ROOT\\0") == S_OK, "start");
            if (!early) fixture.worker = std::thread([&fixture] { fixture.complete(); });
            std::wstring instance;
            require(p.wait(1000, instance) == S_OK && instance == actual, "actual callback identity");
            require(p.start(uuid, L"HTREE\\ROOT\\0") == E_INVALIDARG, "no second create");
            p.close(); p.close();
        }
        require(fixture.creates == 1 && fixture.closes == 1, "exactly one close");
    }
    for (unsigned mode = 0; mode < 7; ++mode) {
        SoftwareFixture fixture; software = &fixture;
        if (mode == 0) fixture.create_result = E_ACCESSDENIED;
        if (mode == 1) { fixture.early = true; fixture.callback_result = E_FAIL; }
        if (mode == 2) { fixture.early = true; fixture.null_id = true; }
        if (mode == 3) fixture.null_handle = true;
        if (mode == 5) { fixture.early = true; fixture.wrong_callback_handle = true; }
        {
            Provision p({create, close_software});
            const auto hr = p.start(uuid, L"HTREE\\ROOT\\0");
            std::wstring instance = L"stale";
            if (mode == 0 || mode == 3) require(hr != S_OK, "create failure/null handle");
            else {
                require(hr == S_OK, "initiation independent of callback result");
                if (mode == 6) {
                    // Callback runs during close; context must outlive drain/join.
                    fixture.worker = std::thread([&fixture] { fixture.complete(); });
                    p.close();
                } else require(p.wait(1, instance) != S_OK && instance.empty(), "callback failure/timeout");
            }
        }
        require(fixture.closes == (mode == 3 ? 0u : 1u), "failure/cancel cleanup");
    }
    SoftwareFixture fixture; software = &fixture;
    { Provision p({create, close_software}); require(p.start(L"bad", L"parent") == E_INVALIDARG, "invalid UUID before API"); }
    require(!fixture.creates && !fixture.closes, "invalid input has no operations");
}

enum class Fault { none, open, status_ioctl, status_short, status_version, status_bytes, status_reserved,
    generation, epoch, running, dirty, reply_ioctl, reply_short, reply_version, reply_bytes, reply_reserved, reply_result, close };
struct DeviceFixture {
    Fault fault = Fault::none;
    Transport transport{};
    unsigned opens = 0, closes = 0, submits = 0;
    DeviceFixture() { transport.initialize(); require(transport.begin(23, 44) == Result::accepted, "fixture admission"); }
} *device = nullptr;
HANDLE open_file() { ++device->opens; return device->fault == Fault::open ? INVALID_HANDLE_VALUE : file_handle(); }
BOOL close_file(HANDLE file) {
    require(file == file_handle(), "close exact opened file");
    ++device->closes;
    (void)device->transport.cleanup(23);
    return device->fault == Fault::close ? FALSE : TRUE;
}
BOOL ioctl(HANDLE file, DWORD code, LPVOID input, DWORD input_bytes, LPVOID output,
           DWORD output_bytes, LPDWORD bytes, LPOVERLAPPED overlapped) {
    auto& f = *device;
    require(file == file_handle() && !overlapped, "same file/synchronous IOCTL");
    if (code == ioctl_capture_status) {
        require(!input && !input_bytes && output_bytes == sizeof(CaptureStatus), "status request bounds");
        if (f.fault == Fault::status_ioctl) return FALSE;
        auto& s = *static_cast<CaptureStatus*>(output);
        s = capture_status_snapshot({}, f.transport); *bytes = sizeof(s);
        if (f.fault == Fault::status_short) --*bytes;
        if (f.fault == Fault::status_version) ++s.version;
        if (f.fault == Fault::status_bytes) --s.bytes;
        if (f.fault == Fault::status_reserved) ++s.reserved;
        if (f.fault == Fault::generation) s.generation = 0;
        if (f.fault == Fault::epoch) s.producer_epoch = 0;
        if (f.fault == Fault::running) s.running = 2;
        if (f.fault == Fault::dirty) s.next_frame = 1;
    } else {
        require(code == ioctl_submit && output_bytes == sizeof(SubmitReply), "submit request bounds");
        ++f.submits;
        if (f.fault == Fault::reply_ioctl) return FALSE;
        const auto result = f.transport.submit(23, input, input_bytes);
        require(result == Result::accepted, "actual shared transport accepts serialized fixture");
        const auto& packet = f.transport.packets[f.transport.next_sequence - 1];
        require(packet.frames == 128 && packet.samples[0] == 0x3e800000 && packet.samples[1] == 0xbe000000 &&
            packet.samples[254] == 0x3e800000 && packet.samples[255] == 0xbe000000, "exact L/R first/tail bits");
        auto& r = *static_cast<SubmitReply*>(output);
        r = {control_version, sizeof(r), static_cast<uint32_t>(result), 0}; *bytes = sizeof(r);
        if (f.fault == Fault::reply_short) --*bytes;
        if (f.fault == Fault::reply_version) ++r.version;
        if (f.fault == Fault::reply_bytes) --r.bytes;
        if (f.fault == Fault::reply_reserved) ++r.reserved;
        if (f.fault == Fault::reply_result) r.result = static_cast<uint32_t>(Result::accepted_overwrite);
    }
    return TRUE;
}
void packet_checks() {
    for (auto fault : {Fault::none, Fault::open, Fault::status_ioctl, Fault::status_short, Fault::status_version,
         Fault::status_bytes, Fault::status_reserved, Fault::generation, Fault::epoch, Fault::running, Fault::dirty,
         Fault::reply_ioctl, Fault::reply_short, Fault::reply_version, Fault::reply_bytes, Fault::reply_reserved,
         Fault::reply_result, Fault::close}) {
        DeviceFixture fixture; device = &fixture; fixture.fault = fault;
        if (fault == Fault::none) {
            const auto report = produce({open_file, ioctl, close_file}, 7, 8);
            require(report.generation == 1 && report.epoch == 44 && report.frames == 1024 && report.packets == 8,
                    "bounded enqueue report");
            require(fixture.submits == 8, "all packets checked");
        } else fails([] { (void)produce({open_file, ioctl, close_file}, 7, 8); });
        require(fixture.opens == 1 && fixture.closes == (fault == Fault::open ? 0u : 1u), "error closes once");
        unsigned expected_submits = 0;
        switch (fault) {
        case Fault::none: case Fault::close: expected_submits = 8; break;
        case Fault::reply_ioctl: case Fault::reply_short: case Fault::reply_version:
        case Fault::reply_bytes: case Fault::reply_reserved: case Fault::reply_result:
            expected_submits = 1; break;
        default: break;
        }
        require(fixture.submits == expected_submits, "exact submissions; no retry after errors");
        for (const auto& p : fixture.transport.packets) for (auto sample : p.samples)
            require(!sample, "injected close clears queued samples");
    }
    DeviceFixture fixture; device = &fixture;
    for (unsigned n : {0u, 9u}) fails([n] { (void)produce({open_file, ioctl, close_file}, 7, n); });
    fails([] { (void)produce({open_file, ioctl, close_file}, 0, 1); });
    require(!fixture.opens, "bounds rejected before open");
    auto& t = fixture.transport;
    for (uint32_t frames : {0u, 129u}) fails([&] { (void)fixture_packet(t, frames, 1); });
    auto p = fixture_packet(t, 1, 2);
    require(t.submit(23, p.data(), header_bytes + 8) == Result::accepted, "minimum packet");
    fails([&] { (void)fixture_packet(t, 1, 1); });
    require(t.submit(23, p.data(), header_bytes + 8) == Result::out_of_order, "replay rejected");
    p = fixture_packet(t, 1, 2); put(p.data() + 16, 99, 8);
    require(t.submit(23, p.data(), header_bytes + 8) == Result::stale, "wrong generation");
    p = fixture_packet(t, 1, 2); put(p.data() + 24, 99, 8);
    require(t.submit(23, p.data(), header_bytes + 8) == Result::stale, "wrong epoch");
    p = fixture_packet(t, 1, 2); put(p.data() + header_bytes, 0x7fc00000, 4);
    require(t.submit(23, p.data(), header_bytes + 8) == Result::malformed, "nonfinite rejected");
    p = fixture_packet(t, 1, 2); put(p.data() + 48, 44100, 4);
    require(t.submit(23, p.data(), header_bytes + 8) == Result::malformed, "rate rejected");
}
}
int producer_selfcheck() {
    lifecycle_checks(); packet_checks();
    std::puts("PASS: INJECTED software-device/IOCTL operations; shared lifecycle/packet/response code only. NEVER actual device calls. OS callbacks/kernel/endpoint/consumer PCM UNPROVED.");
    return 0;
}
