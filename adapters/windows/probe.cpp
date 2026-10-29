#include "audio.hpp"
#include "output.hpp"

#include <windows.h>
#include <audioclient.h>
#include <audioclientactivationparams.h>
#include <mmdeviceapi.h>
#include <mmreg.h>
#include <functiondiscoverykeys_devpkey.h>
#include <ks.h>
#include <ksmedia.h>
#include <avrt.h>
#include <wrl.h>
#include <winternl.h>

#include <chrono>
#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

using Microsoft::WRL::ComPtr;
using namespace mixyard;

std::string utf8(const wchar_t* value) {
    if (!value) return {};
    const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value, -1, nullptr, 0, nullptr, nullptr);
    if (!size) throw std::runtime_error("invalid-utf16: Windows returned invalid text");
    std::string result(static_cast<std::size_t>(size), '\0');
    if (!WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value, -1, result.data(), size, nullptr, nullptr))
        throw std::runtime_error("utf8-conversion-failed");
    result.pop_back(); return result;
}

std::string json(const std::string& value) {
    std::string out = "\"";
    constexpr char hex[] = "0123456789abcdef";
    for (unsigned char c : value) {
        if (c == '"' || c == '\\') { out += '\\'; out += static_cast<char>(c); }
        else if (c < 32) { out += "\\u00"; out += hex[c >> 4]; out += hex[c & 15]; }
        else out += static_cast<char>(c);
    }
    return out + '"';
}

struct Failure : std::runtime_error {
    std::string code;
    Failure(std::string c, std::string message) : std::runtime_error(std::move(message)), code(std::move(c)) {}
};

void checked(HRESULT hr, const char* operation) {
    if (SUCCEEDED(hr)) return;
    std::string code = "windows-api-error", hint;
    if (hr == E_ACCESSDENIED || hr == HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED)) {
        code = "permission-denied"; hint = "; check Windows microphone privacy access for desktop apps";
    } else if (hr == AUDCLNT_E_DEVICE_INVALIDATED || hr == HRESULT_FROM_WIN32(ERROR_NOT_FOUND)) {
        code = "source-unavailable"; hint = "; selected source was lost; no fallback was attempted";
    } else if (hr == AUDCLNT_E_SERVICE_NOT_RUNNING) {
        code = "audio-service-unavailable"; hint = "; Windows Audio is unavailable";
    } else if (hr == AUDCLNT_E_DEVICE_IN_USE) {
        code = "device-in-use"; hint = "; another app may hold exclusive access; shared mode only";
    } else if (hr == AUDCLNT_E_UNSUPPORTED_FORMAT) {
        code = "unsupported-format";
    }
    std::ostringstream message;
    message << operation << " failed, HRESULT=0x" << std::hex << static_cast<std::uint32_t>(hr) << hint;
    throw Failure(code, message.str());
}

struct Handle {
    HANDLE value = nullptr;
    explicit Handle(HANDLE h = nullptr) : value(h) {}
    ~Handle() { if (value && value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
};
struct ComApartment {
    ComApartment() { checked(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "CoInitializeEx"); }
    ~ComApartment() { CoUninitialize(); }
};
struct TaskFree { void operator()(void* value) const { CoTaskMemFree(value); } };

DWORD os_build() {
    using GetVersion = LONG(WINAPI*)(PRTL_OSVERSIONINFOW);
    const auto fn = reinterpret_cast<GetVersion>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "RtlGetVersion"));
    RTL_OSVERSIONINFOW version{}; version.dwOSVersionInfoSize = sizeof(version);
    return fn && fn(&version) == 0 ? version.dwBuildNumber : 0;
}

ComPtr<IMMDeviceEnumerator> enumerator() {
    ComPtr<IMMDeviceEnumerator> result;
    checked(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, IID_PPV_ARGS(&result)), "MMDeviceEnumerator");
    return result;
}

struct Diagnostic { std::string code, message, severity; };
void snapshot() {
    std::vector<std::string> devices;
    std::vector<Diagnostic> diagnostics{
        {"transport-unimplemented", "Capture-to-WAV does not publish virtual microphones or independent mixes", "warning"},
        {"virtual-classification-unknown", "virtual=false means this probe has not identified an endpoint as Mixyard-owned; third-party virtual endpoints are not classified", "warning"}
    };
    bool discovery = false, failed = false;
    const bool process_available = os_build() >= 20348;
    if (!process_available) diagnostics.push_back({"unsupported-OS", "Selected-process loopback requires Windows build 20348 or later", "warning"});
    try {
        ComApartment apartment;
        auto e = enumerator();
        ComPtr<IMMDeviceCollection> collection;
        checked(e->EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE, &collection), "EnumAudioEndpoints");
        UINT count = 0; checked(collection->GetCount(&count), "GetCount");
        for (UINT i = 0; i < count; ++i) {
            try {
                ComPtr<IMMDevice> device; checked(collection->Item(i, &device), "Endpoint Item");
                LPWSTR raw = nullptr; checked(device->GetId(&raw), "GetId");
                std::unique_ptr<wchar_t, TaskFree> id(raw);
                ComPtr<IMMEndpoint> endpoint; checked(device.As(&endpoint), "IMMEndpoint");
                EDataFlow flow; checked(endpoint->GetDataFlow(&flow), "GetDataFlow");
                ComPtr<IPropertyStore> properties; checked(device->OpenPropertyStore(STGM_READ, &properties), "OpenPropertyStore");
                PROPVARIANT name{};
                const HRESULT hr = properties->GetValue(PKEY_Device_FriendlyName, &name);
                std::string label;
                if (SUCCEEDED(hr) && name.vt == VT_LPWSTR) label = utf8(name.pwszVal);
                PropVariantClear(&name); checked(hr, "FriendlyName");
                if (label.empty()) {
                    label = utf8(id.get());
                    diagnostics.push_back({"endpoint-name-unavailable", "Endpoint label unavailable; opaque ID retained", "warning"});
                }
                devices.push_back("{\"id\":" + json(utf8(id.get())) + ",\"name\":" + json(label) +
                    ",\"direction\":" + json(flow == eCapture ? "input" : "output") + ",\"virtual\":false}");
            } catch (const Failure& error) {
                failed = true; diagnostics.push_back({error.code, error.what(), "error"});
            }
        }
        discovery = !failed;
        if (!count) {
            failed = true; discovery = false;
            diagnostics.push_back({"devices-unavailable", "No active Windows audio endpoints were observed", "error"});
        }
    } catch (const Failure& error) {
        failed = true; diagnostics.push_back({error.code, error.what(), "error"});
    } catch (const std::exception& error) {
        failed = true; diagnostics.push_back({"discovery-failed", error.what(), "error"});
    }
    std::cout << "{\"schemaVersion\":1,\"platform\":\"windows\",\"capabilities\":{\"deviceDiscovery\":"
        << (discovery ? "true" : "false") << ",\"processCapture\":" << (process_available && discovery ? "true" : "false")
        << ",\"virtualInputs\":false,\"independentMixes\":false,\"routeRecovery\":false},\"devices\":[";
    for (std::size_t i = 0; i < devices.size(); ++i) std::cout << (i ? "," : "") << devices[i];
    std::cout << "],\"diagnostics\":[";
    for (std::size_t i = 0; i < diagnostics.size(); ++i) {
        const auto& d = diagnostics[i];
        std::cout << (i ? "," : "") << "{\"code\":" << json(d.code) << ",\"message\":" << json(d.message)
                  << ",\"severity\":" << json(d.severity) << "}";
    }
    std::cout << "]}\n";
    if (failed) throw Failure("snapshot-failed", "snapshot contains errors; inspect diagnostics");
}

class Activation final : public Microsoft::WRL::RuntimeClass<
    Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>,
    IActivateAudioInterfaceCompletionHandler, Microsoft::WRL::FtmBase> {
public:
    Handle event{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    HRESULT result = E_PENDING;
    ComPtr<IAudioClient> client;
    STDMETHODIMP ActivateCompleted(IActivateAudioInterfaceAsyncOperation* operation) override {
        ComPtr<IUnknown> object;
        HRESULT activation_hr = E_FAIL;
        result = operation->GetActivateResult(&activation_hr, &object);
        if (SUCCEEDED(result)) result = activation_hr;
        if (SUCCEEDED(result)) result = object.As(&client);
        SetEvent(event.value); return S_OK;
    }
};

ComPtr<IAudioClient> process_client(DWORD pid) {
    if (os_build() < 20348) throw Failure("unsupported-OS", "Process-tree capture requires Windows build 20348 or later");
    AUDIOCLIENT_ACTIVATION_PARAMS params{};
    params.ActivationType = AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK;
    params.ProcessLoopbackParams.TargetProcessId = pid;
    params.ProcessLoopbackParams.ProcessLoopbackMode = PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE;
    PROPVARIANT variant{}; variant.vt = VT_BLOB;
    variant.blob.cbSize = sizeof(params); variant.blob.pBlobData = reinterpret_cast<BYTE*>(&params);
    auto completion = Microsoft::WRL::Make<Activation>();
    if (!completion || !completion->event.value) throw Failure("resource-unavailable", "Could not allocate activation event");
    ComPtr<IActivateAudioInterfaceAsyncOperation> operation;
    checked(ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, __uuidof(IAudioClient),
        &variant, completion.Get(), &operation), "ActivateAudioInterfaceAsync (process tree)");
    // Completion owns its event/client, so a late callback after timeout has no stack pointers.
    const DWORD wait = WaitForSingleObject(completion->event.value, 10000);
    if (wait != WAIT_OBJECT_0) throw Failure("activation-timeout", "Process-loopback activation did not complete within 10 seconds");
    checked(completion->result, "Process-loopback activation");
    return completion->client;
}

Format format_of(const WAVEFORMATEX& wave) {
    bool floating = wave.wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
    bool integer = wave.wFormatTag == WAVE_FORMAT_PCM;
    WORD valid_bits = wave.wBitsPerSample;
    if (wave.wFormatTag == WAVE_FORMAT_EXTENSIBLE && wave.cbSize >= 22) {
        const auto& extended = reinterpret_cast<const WAVEFORMATEXTENSIBLE&>(wave);
        floating = IsEqualGUID(extended.SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        integer = IsEqualGUID(extended.SubFormat, KSDATAFORMAT_SUBTYPE_PCM);
        if (extended.Samples.wValidBitsPerSample > wave.wBitsPerSample)
            throw Failure("unsupported-format", "Valid bits exceed the PCM container");
        valid_bits = extended.Samples.wValidBitsPerSample;
        const DWORD mask = extended.dwChannelMask;
        if (mask && mask != (wave.nChannels == 1 ? SPEAKER_FRONT_CENTER : SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT))
            throw Failure("unsupported-format", "Unhandled channel layout; require mono or front L/R stereo");
    }
    if (!floating && !integer) throw Failure("unsupported-format", "Compressed or unknown endpoint format");
    Format result{wave.nSamplesPerSec, wave.nChannels, wave.wBitsPerSample, floating, valid_bits};
    result.validate();
    if (wave.nBlockAlign != result.block_align() || wave.nAvgBytesPerSec != result.rate * result.block_align())
        throw Failure("unsupported-format", "Invalid block alignment or byte rate");
    return result;
}

unsigned number(const std::wstring& value, unsigned maximum, const char* label) {
    if (value.empty() || value.find_first_not_of(L"0123456789") != std::wstring::npos)
        throw Failure("invalid-argument", std::string(label) + " must be a positive integer");
    std::size_t used;
    const auto parsed = std::stoull(value, &used);
    if (!parsed || parsed > maximum) throw Failure("invalid-argument", std::string(label) + " is out of range");
    return static_cast<unsigned>(parsed);
}

struct Options {
    std::wstring mic, path;
    DWORD pid = 0;
    unsigned seconds = 0;
};
Options options(int argc, wchar_t** argv) {
    Options out;
    for (int i = 2; i < argc; i += 2) {
        if (i + 1 >= argc) throw Failure("invalid-argument", "Each capture option requires a value");
        const std::wstring key = argv[i], value = argv[i+1];
        if (key == L"--mic" && out.mic.empty()) out.mic = value;
        else if (key == L"--pid" && !out.pid) out.pid = number(value, MAXDWORD, "PID");
        else if (key == L"--seconds" && !out.seconds) out.seconds = number(value, 600, "seconds (1-600)");
        else if (key == L"--out" && out.path.empty()) out.path = value;
        else throw Failure("invalid-argument", "Unknown or duplicate capture option");
    }
    if (out.mic.empty() == (out.pid == 0) || out.path.empty() || !out.seconds)
        throw Failure("invalid-argument", "Use exactly one of --mic ID or --pid PID, plus --seconds 1..600 and --out FILE.wav");
    if (out.pid == GetCurrentProcessId()) throw Failure("own-process-rejected", "Cannot capture Mixyard's own process");
    return out;
}

void capture(const Options& opts) {
    ComApartment apartment;
    Handle process;
    ComPtr<IAudioClient> client;
    std::unique_ptr<WAVEFORMATEX, TaskFree> negotiated;
    WAVEFORMATEX process_format{};
    WAVEFORMATEX* wave = nullptr;
    DWORD stream_flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    if (opts.pid) {
        process.value = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, opts.pid);
        if (!process.value) checked(HRESULT_FROM_WIN32(GetLastError()), "Open selected process");
        if (WaitForSingleObject(process.value, 0) != WAIT_TIMEOUT)
            throw Failure("source-unavailable", "Selected process has exited");
        client = process_client(opts.pid);
        WAVEFORMATEX* raw = nullptr;
        const HRESULT mix_hr = client->GetMixFormat(&raw);
        if (mix_hr == E_NOTIMPL) {
            // Virtual process client has no negotiable physical endpoint on these
            // builds. Native engine conversion is required for mixed source rates.
            process_format.wFormatTag = WAVE_FORMAT_IEEE_FLOAT;
            process_format.nChannels = 2; process_format.nSamplesPerSec = 48000;
            process_format.wBitsPerSample = 32; process_format.nBlockAlign = 8;
            process_format.nAvgBytesPerSec = 384000;
            wave = &process_format;
            stream_flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;
        } else {
            checked(mix_hr, "Process GetMixFormat"); negotiated.reset(raw); wave = raw;
        }
        stream_flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    } else {
        auto e = enumerator(); ComPtr<IMMDevice> device;
        checked(e->GetDevice(opts.mic.c_str(), &device), "Get selected endpoint by ID");
        ComPtr<IMMEndpoint> endpoint; checked(device.As(&endpoint), "IMMEndpoint");
        EDataFlow flow; checked(endpoint->GetDataFlow(&flow), "GetDataFlow");
        if (flow != eCapture) throw Failure("invalid-source", "--mic must name a capture endpoint ID");
        DWORD state; checked(device->GetState(&state), "GetState");
        if (state != DEVICE_STATE_ACTIVE) throw Failure("source-unavailable", "Selected capture endpoint is inactive");
        checked(device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(client.GetAddressOf())), "Activate microphone");
        WAVEFORMATEX* raw = nullptr; checked(client->GetMixFormat(&raw), "GetMixFormat");
        negotiated.reset(raw); wave = raw;
    }
    const Format format = format_of(*wave);
    checked(client->Initialize(AUDCLNT_SHAREMODE_SHARED, stream_flags, 0, 0, wave, nullptr), "Initialize shared capture");
    UINT32 buffer_frames = 0;
    checked(client->GetBufferSize(&buffer_frames), "GetBufferSize");
    if (!buffer_frames || buffer_frames > packet_frames * queue_slots)
        throw Failure("unsupported-buffer", "WASAPI buffer exceeds the bounded 65536-frame servicing limit");
    Handle event(CreateEventW(nullptr, FALSE, FALSE, nullptr));
    if (!event.value) checked(HRESULT_FROM_WIN32(GetLastError()), "Create capture event");
    checked(client->SetEventHandle(event.value), "SetEventHandle");
    ComPtr<IAudioCaptureClient> capture_client;
    checked(client->GetService(IID_PPV_ARGS(&capture_client)), "IAudioCaptureClient");
    auto queue = std::make_unique<Queue>(); // Allocate and touch all slots before capture starts.
    auto faults = std::make_unique<FaultQueue>();
    // CREATE_NEW acquires the same handles used for every write; no check/reopen race.
    ExclusiveFileBuffer output_file(opts.path), gap_file(opts.path + L".gaps.jsonl");
    std::ostream output(&output_file), gaps(&gap_file);
    Wav wav(output, format);
    std::atomic<bool> done{false}, writer_failed{false};
    static_assert(std::atomic<bool>::is_always_lock_free);
    std::uint64_t received = 0, dropped = 0, discontinuities = 0, timestamp_errors = 0, silent = 0, fault_metadata_dropped = 0;
    std::string writer_error;
    std::thread writer([&] {
        try {
            for (;;) {
                while (const PacketFault* fault = faults->front()) {
                    write_fault(gaps, *fault); faults->consume();
                    if (!gaps) throw Failure("disk-write-failed", "Gap report write failed");
                }
                const Packet* packet = queue->front();
                if (!packet) {
                    if (done.load(std::memory_order_acquire)) {
                        // Acquire done then re-read queue: do not miss the last published slot.
                        if (queue->front() || faults->front()) continue;
                        const auto start = wav.frames();
                        const auto tail = pad_gap(wav, received);
                        if (tail) gaps << "{\"code\":\"queue-overflow\",\"firstFrame\":" << start << ",\"frames\":" << tail << "}\n";
                        if (fault_metadata_dropped) write_fault_overflow(gaps, fault_metadata_dropped);
                        break;
                    }
                    std::this_thread::sleep_for(std::chrono::milliseconds(1)); continue;
                }
                const auto start = wav.frames();
                const auto gap = pad_gap(wav, packet->first_frame);
                if (gap) gaps << "{\"code\":\"queue-overflow\",\"firstFrame\":" << start << ",\"frames\":" << gap << "}\n";
                wav.append(packet->data.data(), packet->frames);
                queue->consume();
                if (!gaps) throw Failure("disk-write-failed", "Gap report write failed");
            }
            wav.finish(); gaps.flush();
            if (!gaps) throw Failure("disk-write-failed", "Could not flush gap report");
            const bool output_closed = output_file.close(), gaps_closed = gap_file.close();
            if (!output_closed || !gaps_closed) throw Failure("disk-write-failed", "Could not close WAV or gap report");
        } catch (const std::exception& error) {
            writer_error = error.what(); writer_failed.store(true, std::memory_order_release);
        }
    });
    HRESULT capture_hr = client->Start();
    bool process_lost = false;
    DWORD mmcss_index = 0;
    HANDLE mmcss = SUCCEEDED(capture_hr) ? AvSetMmThreadCharacteristicsW(L"Audio", &mmcss_index) : nullptr;
    const auto until = std::chrono::steady_clock::now() + std::chrono::seconds(opts.seconds);
    const auto frame_limit = static_cast<std::uint64_t>(opts.seconds) * format.rate;
    // Packet servicing below does not allocate, log, acquire locks, or touch disk.
    while (SUCCEEDED(capture_hr) && std::chrono::steady_clock::now() < until && received < frame_limit &&
           !writer_failed.load(std::memory_order_acquire)) {
        if (process.value && WaitForSingleObject(process.value, 0) != WAIT_TIMEOUT) { process_lost = true; break; }
        const DWORD wait = WaitForSingleObject(event.value, 50);
        if (wait == WAIT_TIMEOUT) continue;
        if (wait != WAIT_OBJECT_0) { capture_hr = HRESULT_FROM_WIN32(GetLastError()); break; }
        UINT32 available = 0;
        capture_hr = capture_client->GetNextPacketSize(&available);
        // Bounded drain: at most 64 packets per wake, then check deadline/stop again.
        for (unsigned packets = 0; SUCCEEDED(capture_hr) && available && packets < 64 && received < frame_limit; ++packets) {
            BYTE* data = nullptr; UINT32 frames = 0; DWORD flags = 0; UINT64 position = 0, qpc = 0;
            capture_hr = capture_client->GetBuffer(&data, &frames, &flags, &position, &qpc);
            if (FAILED(capture_hr)) break;
            if (frames > buffer_frames || (!(flags & AUDCLNT_BUFFERFLAGS_SILENT) && !data)) {
                capture_client->ReleaseBuffer(frames); capture_hr = E_UNEXPECTED; break;
            }
            discontinuities += (flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) != 0;
            timestamp_errors += (flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR) != 0;
            if (flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY | AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR)) {
                if (!record_fault(*faults, {received, position, qpc, flags})) ++fault_metadata_dropped;
            }
            const auto count = static_cast<UINT32>(std::min<std::uint64_t>(frames, frame_limit - received));
            if (flags & AUDCLNT_BUFFERFLAGS_SILENT) silent += count;
            for (UINT32 offset = 0; offset < count; ) {
                const UINT32 chunk = static_cast<UINT32>(std::min<std::size_t>(count - offset, packet_frames));
                if (!enqueue_chunk(*queue, received + offset,
                    data ? data + offset * format.block_align() : nullptr, chunk, format.block_align(),
                    (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0)) dropped += chunk;
                offset += chunk;
            }
            received += count;
            capture_hr = capture_client->ReleaseBuffer(frames);
            if (SUCCEEDED(capture_hr)) capture_hr = capture_client->GetNextPacketSize(&available);
        }
    }
    const HRESULT stop_hr = client->Stop();
    if (SUCCEEDED(capture_hr) && FAILED(stop_hr)) capture_hr = stop_hr;
    if (mmcss) AvRevertMmThreadCharacteristics(mmcss);
    done.store(true, std::memory_order_release); writer.join();
    const bool degraded = dropped || discontinuities || timestamp_errors || fault_metadata_dropped;
    const bool failed = FAILED(capture_hr) || writer_failed.load() || process_lost || !received;
    std::cout << "{\"command\":\"capture\",\"status\":" << json(failed ? "failed" : degraded ? "degraded" : "captured")
        << ",\"nativeAudioQualified\":false,\"sampleRate\":" << format.rate << ",\"channels\":" << format.channels
        << ",\"inputBits\":" << format.bits << ",\"inputFloat\":" << (format.floating ? "true" : "false")
        << ",\"engineConversion\":" << ((stream_flags & AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM) ? "true" : "false")
        << ",\"wavBits\":" << format.bits << ",\"receivedFrames\":" << received << ",\"writtenFrames\":" << wav.frames()
        << ",\"queueDroppedFrames\":" << dropped << ",\"discontinuities\":" << discontinuities
        << ",\"faultMetadataDroppedRecords\":" << fault_metadata_dropped
        << ",\"timestampErrors\":" << timestamp_errors << ",\"silentFrames\":" << silent
        << ",\"mmcssEnabled\":" << (mmcss ? "true" : "false") << "}\n";
    if (writer_failed.load()) throw Failure("disk-write-failed", writer_error);
    checked(capture_hr, "Capture stream");
    if (process_lost) throw Failure("source-unavailable", "Selected process exited; capture stopped without resolving a replacement PID");
    if (!received) throw Failure("no-audio", "No capture frames arrived; output does not prove audio capture");
    if (degraded) throw Failure("capture-degraded", "Queue drops or WASAPI timing discontinuities occurred; WAV is not a passing recording (see .gaps.jsonl)");
}

int worker_bridge(int argc, wchar_t** argv);
int wmain(int argc, wchar_t** argv) {
    try {
        if (argc >= 2 && std::wstring(argv[1]) == L"bridge") return worker_bridge(argc, argv);
        if (argc == 2 && std::wstring(argv[1]) == L"snapshot") { snapshot(); return 0; }
        if (argc >= 2 && std::wstring(argv[1]) == L"capture") { capture(options(argc, argv)); return 0; }
        std::cerr << "Usage: mixyard-windows-probe snapshot\n"
                     "       mixyard-windows-probe bridge --mic ENDPOINT_ID --render ENDPOINT_ID --seconds 1..600\n"
                     "       mixyard-windows-probe capture (--mic ENDPOINT_ID | --pid PID) --seconds 1..600 --out FILE.wav\n";
        return 2;
    } catch (const OutputError& error) {
        std::cerr << error.code << ": " << error.what() << '\n'; return 1;
    } catch (const Failure& error) {
        std::cerr << error.code << ": " << error.what() << '\n'; return 1;
    } catch (const std::exception& error) {
        std::cerr << "probe-failed: " << error.what() << '\n'; return 1;
    }
}
