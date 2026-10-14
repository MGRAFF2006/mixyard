#include "audio.hpp"
#include <iostream>
#include <exception>
#include <iterator>
#include <memory>
#include <sstream>
#include <thread>
#ifdef _WIN32
#include "output.hpp"
#include <filesystem>
#include <fstream>
#endif

void require(bool value) {
    if (!value) throw std::runtime_error("model assertion failed");
}

void check_fault_drops() {
    using namespace mixyard;
    auto pcm = std::make_unique<Queue>(); FaultQueue faults;
    for (std::size_t i = 0; i < queue_slots; ++i)
        require(enqueue_chunk(*pcm, i * packet_frames, nullptr, packet_frames, 4, true));
    const std::uint64_t start = queue_slots * packet_frames;
    // A flagged 2048-frame packet: first chunk drops, then the writer frees a slot.
    require(record_fault(faults, {start, 123456, 987654, 5}));
    require(!enqueue_chunk(*pcm, start, nullptr, packet_frames, 4, true));
    pcm->consume();
    require(enqueue_chunk(*pcm, start + packet_frames, nullptr, packet_frames, 4, true));
    // Next flagged packet drops completely, including both chunks.
    require(record_fault(faults, {start + 2048, 125504, 988000, 1}));
    require(!enqueue_chunk(*pcm, start + 2048, nullptr, packet_frames, 4, true));
    require(!enqueue_chunk(*pcm, start + 3072, nullptr, packet_frames, 4, true));
    std::ostringstream report;
    while (auto fault = faults.front()) { write_fault(report, *fault); faults.consume(); }
    require(report.str() ==
        "{\"code\":\"wasapi-packet-flags\",\"firstFrame\":65536,\"flags\":5,\"devicePosition\":123456,\"qpc100ns\":987654,\"missingFrames\":null}\n"
        "{\"code\":\"wasapi-packet-flags\",\"firstFrame\":67584,\"flags\":1,\"devicePosition\":125504,\"qpc100ns\":988000,\"missingFrames\":null}\n");
    // Metadata storage also stays bounded and never replaces unread records.
    for (std::size_t i = 0; i < queue_slots; ++i) require(record_fault(faults, {i, i + 100, i + 200, 1}));
    require(!record_fault(faults, {999, 999, 999, 5}));
    for (std::size_t i = 0; i < queue_slots; ++i) {
        const auto fault = faults.front();
        require(fault && fault->first_frame == i && fault->device_position == i + 100 && fault->qpc_100ns == i + 200);
        faults.consume();
    }
    require(!faults.front());
    require(record_fault(faults, {1000, 2000, 3000, 4})); // Wrap after overflow.
    require(faults.front()->first_frame == 1000); faults.consume();
    std::ostringstream overflow;
    write_fault_overflow(overflow, 1);
    require(overflow.str() == "{\"code\":\"fault-metadata-overflow\",\"records\":1,\"firstFrame\":null,\"missingFrames\":null}\n");
}

#ifdef _WIN32
void check_exclusive_outputs() {
    using namespace mixyard;
    namespace fs = std::filesystem;
    const auto directory = fs::temp_directory_path() /
        ("mixyard-output-check-" + std::to_string(GetCurrentProcessId()) + "-" + std::to_string(GetTickCount64()));
    require(fs::create_directory(directory));
    struct Cleanup {
        fs::path path;
        ~Cleanup() { std::error_code ignored; fs::remove_all(path, ignored); }
    } cleanup{directory};
    const auto path = directory / "race.wav", gap_path = directory / "pair.wav.gaps.jsonl";
    auto read = [](const fs::path& file) {
        std::ifstream in(file, std::ios::binary);
        return std::string(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
    };
    auto sentinel = [](const fs::path& file) {
        ExclusiveFileBuffer buffer(file.wstring()); std::ostream out(&buffer);
        out << "sentinel recording"; out.flush(); require(out.good()); require(buffer.close());
    };
    auto refused = [](const fs::path& file) {
        try { ExclusiveFileBuffer buffer(file.wstring()); }
        catch (const OutputError& error) { require(error.code == "output-exists"); return; }
        require(false);
    };
    require(!fs::exists(path));
    // Deterministically place a competing creation between observation and open.
    std::exception_ptr race_error;
    std::thread competitor([&] { try { sentinel(path); } catch (...) { race_error = std::current_exception(); } });
    competitor.join(); if (race_error) std::rethrow_exception(race_error);
    refused(path); require(read(path) == "sentinel recording");
    {
        ExclusiveFileBuffer first((directory / "pair.wav").wstring());
        require(!fs::exists(gap_path));
        sentinel(gap_path); refused(gap_path);
        require(read(gap_path) == "sentinel recording");
        require(first.close());
    }
    require(read(directory / "pair.wav").empty());
    // WAV seeks/finalizes through the exclusive handle, without reopening it.
    ExclusiveFileBuffer buffer((directory / "success.wav").wstring()); std::ostream output(&buffer);
    std::ostringstream expected(std::ios::binary);
    const Format format{48000, 2, 32, true};
    Wav native(output, format), model(expected, format);
    const std::uint8_t samples[]{1, 2, 3, 4, 5, 6, 7, 8};
    native.append(samples, 1); model.append(samples, 1);
    native.finish(); model.finish(); require(buffer.close());
    require(read(directory / "success.wav") == expected.str());
    // A closed/failed handle must surface a stream write error.
    bool failed = false;
    try { native.append(samples, 1); } catch (const std::runtime_error&) { failed = true; }
    require(failed);
    std::cout << "PASS Windows exclusive output: competing creation/second-file sentinel preservation, handle-backed WAV seek/flush, write failure\n";
}
#endif

int main() {
    using namespace mixyard;
    try {
        check_fault_drops();
#ifdef _WIN32
        check_exclusive_outputs();
#endif
        Queue queue;
        require(queue.front() == nullptr);
        for (std::size_t i = 0; i < queue_slots; ++i) {
            auto p = queue.reserve(); require(p != nullptr);
            p->first_frame = i; queue.publish();
        }
        require(queue.reserve() == nullptr); // Overflow never overwrites unread audio.
        for (std::size_t i = 0; i < queue_slots; ++i) {
            require(queue.front()->first_frame == i); queue.consume();
        }
        std::thread producer([&] {
            for (std::uint64_t i = 0; i < 100000; ++i) {
                Packet* p;
                while (!(p = queue.reserve())) std::this_thread::yield();
                p->first_frame = i;
                p->data[0] = static_cast<std::uint8_t>(i);
                queue.publish();
            }
        });
        bool ordered = true;
        for (std::uint64_t i = 0; i < 100000; ++i) {
            const Packet* p;
            while (!(p = queue.front())) std::this_thread::yield();
            ordered = ordered && p->first_frame == i && p->data[0] == static_cast<std::uint8_t>(i);
            queue.consume();
        }
        producer.join(); require(ordered);
        Format f{48000, 1, 32, true};
        const float samples[]{-1, -0.5f, 0, 0.5f, 1, 0.1234567f, 0.999999f};
        std::ostringstream out(std::ios::binary);
        Wav wav(out, f);
        wav.append(reinterpret_cast<const std::uint8_t*>(samples), 7);
        require(pad_gap(wav, 10) == 3); // Includes dropped tail, even without a next packet.
        wav.finish();
        auto bytes = out.str();
        require(bytes.size() == 120 && bytes.substr(0, 4) == "RIFF" && bytes.substr(72, 4) == "data");
        require(static_cast<unsigned char>(bytes[4]) == 112 && static_cast<unsigned char>(bytes[76]) == 40);
        require(bytes.substr(80, sizeof(samples)) == std::string(reinterpret_cast<const char*>(samples), sizeof(samples)));
        require(bytes.substr(108) == std::string(12, '\0')); // Overflow positions remain silence, reported separately.
        require(bytes.substr(60, 4) == "fact" && static_cast<unsigned char>(bytes[68]) == 10);
        bool rejected = false;
        try { Format{48000, 6, 32, true}.validate(); } catch (const std::runtime_error&) { rejected = true; }
        require(rejected);
        std::ostringstream stereo_out(std::ios::binary);
        Wav stereo(stereo_out, Format{44100, 2, 16, false});
        const std::uint8_t pair[]{0, 64, 0, 192};
        stereo.append(pair, 1); stereo.finish();
        require(stereo_out.str().substr(68) == std::string(reinterpret_cast<const char*>(pair), 4));
        std::ostringstream packed_out(std::ios::binary);
        Wav packed(packed_out, Format{48000, 1, 24, false});
        const std::uint8_t packed_sample[]{0x12, 0x34, 0x56};
        packed.append(packed_sample, 1); packed.finish();
        require(packed_out.str().size() == 72 && static_cast<unsigned char>(packed_out.str()[4]) == 64);
        require(packed_out.str().substr(68, 3) == std::string(reinterpret_cast<const char*>(packed_sample), 3));
        require(packed_out.str()[71] == 0 && static_cast<unsigned char>(packed_out.str()[64]) == 3);
        std::ostringstream valid_out(std::ios::binary);
        Wav valid(valid_out, Format{48000, 1, 32, false, 24});
        valid.finish(); require(static_cast<unsigned char>(valid_out.str()[38]) == 24);
        std::ostringstream gaps_out(std::ios::binary);
        Wav gaps(gaps_out, Format{44100, 2, 16, false});
        gaps.append(pair, 1); require(pad_gap(gaps, 3) == 2);
        gaps.append(pair, 1); require(pad_gap(gaps, 5) == 1);
        gaps.finish();
        require(gaps_out.str().substr(72, 8) == std::string(8, '\0'));
        require(gaps_out.str().substr(80, 4) == std::string(reinterpret_cast<const char*>(pair), 4));
        require(gaps_out.str().substr(84, 4) == std::string(4, '\0'));
        rejected = false;
        try { gaps.append(nullptr, UINT32_MAX); } catch (const std::runtime_error&) { rejected = true; }
        require(rejected); // Size rejection happens before any huge write/allocation.
        std::ostringstream broken_out(std::ios::binary);
        Wav broken(broken_out, Format{44100, 2, 16, false});
        broken_out.setstate(std::ios::badbit);
        rejected = false;
        try { broken.append(pair, 1); } catch (const std::runtime_error&) { rejected = true; }
        require(rejected);
        std::cout << "PASS portable model: bounded queue/full/wrap/concurrency, packet faults across partial/complete PCM drops and metadata overflow, exact PCM/float WAV payload, headers/fact/padding/valid-bits, gap silence, stereo, format/size/write-failure rejection\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n'; return 1;
    }
}
