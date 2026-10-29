// SPDX-License-Identifier: MIT
// Portable schema-2 writer shared by the two native read-only helpers.
#pragma once
#include <cstdint>
#include <cstdio>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace mixyard::inventory {
constexpr std::size_t max_items = 1024;
constexpr std::size_t max_identifier_bytes = 4096;
constexpr std::size_t max_property_bytes = 64 * 1024;
constexpr std::size_t max_result_bytes = 1024 * 1024; // Includes trailing newline.

struct Unavailable : std::runtime_error {
    Unavailable() : std::runtime_error("native-inventory-unavailable") {}
};

inline void identifier(const std::string& value) {
    if (value.empty() || value.size() > max_identifier_bytes) throw Unavailable();
    bool nonblank = false;
    // Reject malformed UTF-8 rather than replacing bytes in an opaque selector.
    for (std::size_t i = 0; i < value.size();) {
        const auto c = static_cast<unsigned char>(value[i++]);
        if (c == 0) throw Unavailable();
        if (c < 0x80) {
            if (c != ' ' && !(c >= 9 && c <= 13)) nonblank = true;
            continue;
        }
        nonblank = true;
        unsigned following = 0;
        std::uint32_t scalar = 0, minimum = 0;
        if (c >= 0xc2 && c <= 0xdf) { following = 1; scalar = c & 0x1f; minimum = 0x80; }
        else if (c >= 0xe0 && c <= 0xef) { following = 2; scalar = c & 0x0f; minimum = 0x800; }
        else if (c >= 0xf0 && c <= 0xf4) { following = 3; scalar = c & 7; minimum = 0x10000; }
        else throw Unavailable();
        if (value.size() - i < following) throw Unavailable();
        while (following--) {
            const auto next = static_cast<unsigned char>(value[i++]);
            if ((next & 0xc0) != 0x80) throw Unavailable();
            scalar = (scalar << 6) | (next & 0x3f);
        }
        if (scalar < minimum || scalar > 0x10ffff || (scalar >= 0xd800 && scalar <= 0xdfff))
            throw Unavailable();
    }
    if (!nonblank) throw Unavailable();
}

inline std::string quoted(const std::string& value) {
    identifier(value);
    std::string result = "\"";
    constexpr char hex[] = "0123456789abcdef";
    for (const unsigned char c : value) {
        if (c == '"' || c == '\\') { result += '\\'; result += static_cast<char>(c); }
        else if (c < 32) { result += "\\u00"; result += hex[c >> 4]; result += hex[c & 15]; }
        else result += static_cast<char>(c);
    }
    return result + '"';
}

struct Format { std::uint16_t channels; std::uint32_t rate; };
inline std::optional<Format> format(std::uint32_t channels, std::uint32_t rate) {
    if ((channels != 1 && channels != 2) || !rate) return std::nullopt;
    return Format{static_cast<std::uint16_t>(channels), rate};
}
inline std::string format_json(const std::optional<Format>& f) {
    if (!f) return "null";
    if (!format(f->channels, f->rate)) throw Unavailable();
    return "{\"channels\":" + std::to_string(f->channels) +
        ",\"sampleRateHz\":" + std::to_string(f->rate) + "}";
}

class Snapshot {
    std::string platform_;
    std::vector<std::string> sources_, outputs_;
    std::size_t observation_bytes_ = 0;
    void add(std::vector<std::string>& catalog, std::string observation) {
        if (catalog.size() == max_items || observation.size() > max_result_bytes - observation_bytes_)
            throw Unavailable();
        observation_bytes_ += observation.size();
        catalog.push_back(std::move(observation));
    }
    std::string observed_id(const char* kind, std::size_t index) const {
        return quoted(platform_ + ":" + kind + ":" + std::to_string(index));
    }
public:
    explicit Snapshot(std::string platform) : platform_(std::move(platform)) {
        if (platform_ != "windows" && platform_ != "macos") throw Unavailable();
    }
    void mic(const std::string& uid, std::optional<Format> f) {
        add(sources_, "{\"observedId\":" + observed_id("source", sources_.size()) +
            ",\"selector\":{\"kind\":\"mic\",\"deviceUid\":" + quoted(uid) +
            "},\"origin\":\"external\",\"format\":" + format_json(f) + "}");
    }
    void app(const std::string& bundle, bool producer) {
        add(sources_, "{\"observedId\":" + observed_id("source", sources_.size()) +
            ",\"selector\":{\"kind\":\"app\",\"applicationId\":" + quoted(bundle) +
            "},\"origin\":" + (producer ? "\"mixyard-producer\"" : "\"external\"") + ",\"format\":null}");
    }
    void output(const std::string& uid, std::optional<Format> f) {
        add(outputs_, "{\"observedId\":" + observed_id("output", outputs_.size()) +
            ",\"target\":{\"kind\":\"device\",\"deviceUid\":" + quoted(uid) +
            "},\"format\":" + format_json(f) + "}");
    }
    std::string json() const {
        std::string out;
        const auto append = [&out](const std::string& part) {
            if (part.size() > max_result_bytes - out.size()) throw Unavailable();
            out += part;
        };
        append("{\"schemaVersion\":2,\"revision\":0,\"platform\":" + quoted(platform_) +
               ",\"evidence\":\"native-discovery\",\"sources\":[");
        const auto array = [&append](const std::vector<std::string>& items) {
            bool first = true;
            for (const auto& item : items) { if (!first) append(","); first = false; append(item); }
        };
        array(sources_);
        append("],\"outputs\":["); array(outputs_);
        append("],\"mediaSessions\":[],\"limits\":{\"maxExportedMixes\":null,\"maxOutputRoutes\":null,\"maxVirtualInputs\":null}}\n");
        return out;
    }
};

inline int emit(const Snapshot& result) {
    const auto bytes = result.json(); // Complete and bound before writing anything.
    if (std::fwrite(bytes.data(), 1, bytes.size(), stdout) != bytes.size() || std::fflush(stdout) != 0)
        return 1;
    return 0;
}
inline int unavailable(const char* platform) noexcept {
    // No OS error messages, selectors or exception text reach diagnostics.
    std::fputs("native-inventory-unavailable\n", stderr);
    try { emit(Snapshot(platform)); } catch (...) { /* stdout may be unusable. */ }
    return 1;
}
} // namespace mixyard::inventory
