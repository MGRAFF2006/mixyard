// SPDX-License-Identifier: MIT
#pragma once
#include "host_link.hpp"
#include "../native_worker.hpp"
#include <memory>

namespace mixyard::macos {
struct Selection {
    AudioObjectID process = kAudioObjectUnknown;
    pid_t observedPid = 0;
    // Consent is for this exact observed process and explicit stereo tap.
    // It never asserts that macOS permission was granted, nor bypasses TCC.
    bool consentToStereoTap = false;
    std::vector<AudioObjectID> excludedProcesses;
};
// Owns one selected-process tap, private aggregate and capture-only IOProc.
// open() observes actual UID/ASBD before the host request is constructed. No
// physical clock is qualified, so production cannot currently acquire Timeline.
class Capture {
    struct Impl;
    std::unique_ptr<Impl> impl_;
    explicit Capture(std::unique_ptr<Impl>);
public:
    static std::unique_ptr<Capture> open(Selection, worker::Binding, Timeline);
    ~Capture();
    Capture(const Capture&) = delete;
    Capture& operator=(const Capture&) = delete;
    worker::Queue& queue() noexcept;
    const std::string& observedId() const noexcept;
    void start();
    // For authority/consent revocation, cancel the HostLink and revoke its
    // output Workers first. Stopping this source alone is source loss, not
    // revocation of already queued derived output PCM.
    bool revoke() noexcept;
    bool close() noexcept;
};
} // namespace mixyard::macos
