// SPDX-License-Identifier: MIT
#include "protocol.hpp"
#include <windows.h>
#include <audioclient.h>
#include <mmdeviceapi.h>
#include <mmreg.h>
#include <ks.h>
#include <ksmedia.h>
#include <wrl.h>
#include <fcntl.h>
#include <io.h>
#include <cstring>
#include <memory>

using Microsoft::WRL::ComPtr;
namespace inv = mixyard::inventory;
static void checked(HRESULT status) { if (FAILED(status)) throw inv::Unavailable(); }
struct TaskFree { void operator()(void* p) const noexcept { CoTaskMemFree(p); } };
struct Apartment {
    Apartment() { checked(CoInitializeEx(nullptr, COINIT_MULTITHREADED)); }
    ~Apartment() { CoUninitialize(); }
};

static std::string endpoint_id(const wchar_t* text) {
    if (!text) throw inv::Unavailable();
    std::size_t length = 0;
    while (length <= inv::max_identifier_bytes && text[length]) ++length;
    if (!length || length > inv::max_identifier_bytes) throw inv::Unavailable();
    const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text,
        static_cast<int>(length), nullptr, 0, nullptr, nullptr);
    if (size <= 0 || size > static_cast<int>(inv::max_identifier_bytes)) throw inv::Unavailable();
    std::string result(static_cast<std::size_t>(size), '\0');
    if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, static_cast<int>(length),
        result.data(), size, nullptr, nullptr) != size) throw inv::Unavailable();
    inv::identifier(result);
    return result;
}

static std::optional<inv::Format> query_format(IMMDevice* device) {
    // Activation and GetMixFormat only; no Initialize, stream or capture service.
    ComPtr<IAudioClient> client;
    if (FAILED(device->Activate(__uuidof(IAudioClient), CLSCTX_INPROC_SERVER, nullptr,
        reinterpret_cast<void**>(client.GetAddressOf())))) return std::nullopt;
    WAVEFORMATEX* raw = nullptr;
    const auto status = client->GetMixFormat(&raw);
    std::unique_ptr<WAVEFORMATEX, TaskFree> owned(raw);
    if (FAILED(status) || !raw) return std::nullopt;
    WORD encoding = raw->wFormatTag;
    if (encoding == WAVE_FORMAT_EXTENSIBLE) {
        if (raw->cbSize < sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX)) return std::nullopt;
        const auto* extended = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(raw);
        if (IsEqualGUID(extended->SubFormat, KSDATAFORMAT_SUBTYPE_PCM)) encoding = WAVE_FORMAT_PCM;
        else if (IsEqualGUID(extended->SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT)) encoding = WAVE_FORMAT_IEEE_FLOAT;
        else return std::nullopt;
        if (!extended->Samples.wValidBitsPerSample || extended->Samples.wValidBitsPerSample > raw->wBitsPerSample)
            return std::nullopt;
    }
    const bool pcm = encoding == WAVE_FORMAT_PCM &&
        (raw->wBitsPerSample == 8 || raw->wBitsPerSample == 16 || raw->wBitsPerSample == 24 || raw->wBitsPerSample == 32);
    const bool floating = encoding == WAVE_FORMAT_IEEE_FLOAT && (raw->wBitsPerSample == 32 || raw->wBitsPerSample == 64);
    if (!pcm && !floating) return std::nullopt;
    const auto f = inv::format(raw->nChannels, raw->nSamplesPerSec);
    if (!f || raw->nBlockAlign != raw->nChannels * (raw->wBitsPerSample / 8) ||
        static_cast<std::uint64_t>(raw->nAvgBytesPerSec) != static_cast<std::uint64_t>(raw->nSamplesPerSec) * raw->nBlockAlign)
        return std::nullopt;
    return f;
}

static inv::Snapshot snapshot() {
    Apartment apartment;
    inv::Snapshot result("windows");
    ComPtr<IMMDeviceEnumerator> enumerator;
    checked(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_INPROC_SERVER,
        IID_PPV_ARGS(enumerator.GetAddressOf())));
    ComPtr<IMMDeviceCollection> collection;
    checked(enumerator->EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE, &collection));
    UINT count = 0;
    checked(collection->GetCount(&count));
    if (count > inv::max_items) throw inv::Unavailable();
    for (UINT i = 0; i < count; ++i) {
        ComPtr<IMMDevice> device;
        checked(collection->Item(i, &device));
        LPWSTR raw = nullptr;
        const auto status = device->GetId(&raw);
        std::unique_ptr<wchar_t, TaskFree> owned(raw);
        checked(status);
        const auto uid = endpoint_id(owned.get());
        ComPtr<IMMEndpoint> endpoint;
        checked(device.As(&endpoint));
        EDataFlow flow = eAll;
        checked(endpoint->GetDataFlow(&flow));
        // Device loss during enumeration invalidates the whole observation.
        DWORD state = 0;
        checked(device->GetState(&state));
        if (state != DEVICE_STATE_ACTIVE) throw inv::Unavailable();
        const auto f = query_format(device.Get());
        if (flow == eCapture) result.mic(uid, f);
        else if (flow == eRender) result.output(uid, f);
        else throw inv::Unavailable();
    }
    return result;
}

int main(int argc, char** argv) {
    // The wire bound includes the LF byte. CRT text mode expands it to CRLF,
    // including in fixed diagnostics; use exact bytes before any stdio output.
    if (_setmode(_fileno(stdout), _O_BINARY) == -1 || _setmode(_fileno(stderr), _O_BINARY) == -1)
        return 1;
    if (argc != 2 || std::strcmp(argv[1], "snapshot") != 0) {
        std::fputs("native-inventory-invalid-arguments\n", stderr);
        return 2;
    }
    try { return inv::emit(snapshot()); }
    catch (...) { return inv::unavailable("windows"); }
}
