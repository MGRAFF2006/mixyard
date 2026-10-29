// SPDX-License-Identifier: MIT
#include "protocol.hpp"
#include <cstdlib>
#include <iostream>

namespace inv = mixyard::inventory;
static void require(bool ok) { if (!ok) throw std::runtime_error("inventory-check-failed"); }
template<class F> static void rejected(F action) {
    bool failed = false;
    try { action(); } catch (const inv::Unavailable&) { failed = true; }
    require(failed);
}
static void checks() {
    const std::string uid = "opaque:\"\\\n\t\x01\xc3\xa4\xf0\x9f\x8e\xa7";
    inv::Snapshot result("macos");
    result.mic(uid, inv::format(1, 44100));
    result.mic(uid, std::nullopt); // Duplicate selectors must not be collapsed.
    result.app("org.example.player", false);
    result.app("org.example.player", true);
    result.output(uid, inv::format(2, 96000));
    const auto json = result.json();
    require(json.find("opaque:\\\"\\\\\\u000a\\u0009\\u0001") != std::string::npos);
    require(json.find("macos:source:0") != std::string::npos && json.find("macos:source:3") != std::string::npos);
    require(json.find("\"format\":null") != std::string::npos);
    require(json.find("\"channels\":1,\"sampleRateHz\":44100") != std::string::npos);
    require(!inv::format(0, 48000) && !inv::format(6, 48000) && !inv::format(2, 0));
    rejected([] { inv::Snapshot invalid("linux"); });
    for (const auto& bad : std::vector<std::string>{"", " \t\r\n", std::string("a\0b", 3),
         "\xc0\xaf", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\xe2\x82", "\x80"})
        rejected([&] { inv::identifier(bad); });
    inv::identifier(std::string(inv::max_identifier_bytes, 'x'));
    rejected([] { inv::identifier(std::string(inv::max_identifier_bytes + 1, 'x')); });
    inv::Snapshot count("windows");
    for (std::size_t i = 0; i < inv::max_items; ++i) count.mic("same-uid", std::nullopt);
    require(count.json().size() < inv::max_result_bytes);
    rejected([&] { count.mic("same-uid", std::nullopt); });
    inv::Snapshot bytes("windows");
    rejected([&] {
        // Escaping controls expands identifiers; the serialized-byte bound wins.
        for (std::size_t i = 0; i < inv::max_items; ++i)
            bytes.output(std::string(inv::max_identifier_bytes, '\x01'), std::nullopt);
        bytes.json();
    });
    rejected([&] { result.output("uid", inv::Format{3, 48000}); });
}

int main(int argc, char** argv) {
    try {
        checks();
        if (argc == 2 && std::string(argv[1]) == "fixture") {
            inv::Snapshot fixture("macos");
            fixture.mic("opaque:\"\\\n\xc3\xa4\xf0\x9f\x8e\xa7", inv::format(1, 44100));
            fixture.mic("opaque:\"\\\n\xc3\xa4\xf0\x9f\x8e\xa7", std::nullopt);
            fixture.app("org.example.player", false);
            fixture.app("org.example.player", false);
            fixture.output("opaque-output", inv::format(2, 96000));
            return inv::emit(fixture);
        }
        require(argc == 1);
        std::cout << "native inventory writer checks passed (portable model only)\n";
        return 0;
    } catch (...) { std::cerr << "native inventory writer check failed\n"; return 1; }
}
