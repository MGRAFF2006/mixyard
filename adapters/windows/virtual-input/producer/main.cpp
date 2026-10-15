// SPDX-License-Identifier: MIT
#include "harness.hpp"
#include <iostream>

int producer_selfcheck();
namespace {
using namespace mixyard_producer;
HANDLE open_capture() {
    return CreateFileW(capture_path, GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL, nullptr);
}
BOOL ioctl_capture(HANDLE file, DWORD code, LPVOID input, DWORD input_bytes,
                   LPVOID output, DWORD output_bytes, LPDWORD returned, LPOVERLAPPED overlapped) {
    return DeviceIoControl(file, code, input, input_bytes, output, output_bytes, returned, overlapped);
}
BOOL close_capture(HANDLE file) { return CloseHandle(file); }
uint64_t number(const std::wstring& value, uint64_t maximum) {
    require(!value.empty() && value.find_first_not_of(L"0123456789") == std::wstring::npos, "invalid number");
    const auto parsed = std::stoull(value);
    require(parsed && parsed <= maximum, "number out of range");
    return parsed;
}
void check_hr(HRESULT hr, const char* operation) {
    if (hr != S_OK) throw std::runtime_error(std::string(operation) + " HRESULT=" +
                                            std::to_string(static_cast<unsigned long>(hr)));
}
}
int wmain(int argc, wchar_t** argv) {
    using namespace mixyard_producer;
    try {
        // This branch never constructs a native operation table or calls device APIs.
        if (argc == 2 && std::wstring(argv[1]) == L"--selfcheck") return producer_selfcheck();
        if (argc == 10 && std::wstring(argv[1]) == L"guest-provision" &&
            std::wstring(argv[2]) == L"--authorized-snapshot-guest" &&
            std::wstring(argv[3]) == L"--instance" && std::wstring(argv[5]) == L"--parent" &&
            std::wstring(argv[7]) == L"--hold-ms" && std::wstring(argv[9]) == L"--enumeration-only") {
            const auto hold = static_cast<DWORD>(number(argv[8], 30000));
            require(valid_uuid(argv[4]), "instance must be an explicit lowercase nonzero UUID");
            Provision device({SwDeviceCreate, SwDeviceClose});
            check_hr(device.start(argv[4], argv[6]), "SwDeviceCreate initiation");
            std::wcout << L"creation=initiated; driver-start/MMDevice/delivery=unobserved\n";
            std::wstring instance;
            check_hr(device.wait(5000, instance), "enumeration callback (timeout cancels on close)");
            std::wcout << L"enumeration=callback-success; actualDeviceInstanceId=" << instance
                       << L"\nMMDeviceMapping=BLOCKED; endpoint/PCM=unobserved\n" << std::flush;
            Sleep(hold);
            device.close();
            std::wcout << L"handle=closed; PnP-removal-completion=unobserved\n";
            return 0;
        }
        if (argc == 10 && std::wstring(argv[1]) == L"guest-produce" &&
            std::wstring(argv[2]) == L"--authorized-snapshot-guest" &&
            std::wstring(argv[3]) == L"--device" && std::wstring(argv[4]) == capture_path &&
            std::wstring(argv[5]) == L"--revision" && std::wstring(argv[7]) == L"--packets" &&
            std::wstring(argv[9]) == L"--unmapped-global-control") {
            const auto revision = number(argv[6], max_u64);
            const auto packets = static_cast<unsigned>(number(argv[8], retained_packets));
            const auto report = produce({open_capture, ioctl_capture, close_capture}, revision, packets);
            std::cout << "enqueue=accepted; generation=" << report.generation << "; epoch=" << report.epoch
                      << "; packets=" << report.packets << "; frames=" << report.frames
                      << "; handle=closed/queue-revoked; actual-delivery=UNPROVED; MMDevice=UNMAPPED\n";
            return 0;
        }
        std::cerr << "Source artifact: native device execution BLOCKED under current authorization.\n"
            "Safe: mixyard-producer --selfcheck\n"
            "Future separately authorized snapshot guest ONLY:\n"
            "  guest-provision --authorized-snapshot-guest --instance UUID --parent PNP_ID --hold-ms 1..30000 --enumeration-only\n"
            "  guest-produce --authorized-snapshot-guest --device \\\\.\\Global\\MixyardCaptureIngress --revision N --packets 1..8 --unmapped-global-control\n";
        return 2;
    } catch (const std::exception& error) {
        std::cerr << "harness-failed: " << error.what() << "; no fallback or retry\n";
        return 1;
    }
}
