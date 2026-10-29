// SPDX-License-Identifier: MIT
#include "output_only.hpp"
// This file exists only for a production-symbol link check. Never execute it:
// the compiler must retain the real open/start/revoke/close implementation.
int main(int argc, char** argv) {
    if (argc != 2) return 2;
    mixyard::worker::Queue q({48000, {"CA:1", "CA:2"}, mixyard::worker::Precision::Float32},
        {1, 1}, 1, {mixyard::worker::Origin::AdaptedEngine, "link-check-not-audio-proof"});
    auto w = mixyard::output::Worker::open(argv[1], q);
    w->start();
    return w->revoke() && w->close() ? 0 : 1;
}
