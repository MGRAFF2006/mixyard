#include "audio.hpp"
#include <iostream>
#include <sstream>
#include <thread>

void require(bool value) {
    if (!value) throw std::runtime_error("model assertion failed");
}

int main() {
    using namespace mixyard;
    try {
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
        std::cout << "PASS portable model: bounded queue/full/wrap/concurrency, exact PCM/float WAV payload, headers/fact/padding/valid-bits, gap silence, stereo, format/size/write-failure rejection\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n'; return 1;
    }
}
