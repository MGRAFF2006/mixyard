// SPDX-License-Identifier: MIT
#pragma once
#include <AudioUnit/AudioUnit.h>
#include <CoreAudio/CoreAudio.h>
#include <dispatch/dispatch.h>

namespace mixyard::output::detail {
// Real SDK signatures, injected only by the test build. Production has no mutable
// API override, environment switch or alternate native identity.
struct Api {
    decltype(&AudioComponentFindNext) find = AudioComponentFindNext;
    decltype(&AudioComponentInstanceNew) create = AudioComponentInstanceNew;
    decltype(&AudioComponentInstanceDispose) dispose = AudioComponentInstanceDispose;
    decltype(&AudioUnitSetProperty) set = AudioUnitSetProperty;
    decltype(&AudioUnitGetProperty) get = AudioUnitGetProperty;
    decltype(&AudioUnitInitialize) initialize = AudioUnitInitialize;
    decltype(&AudioUnitUninitialize) uninitialize = AudioUnitUninitialize;
    decltype(&AudioOutputUnitStart) start = AudioOutputUnitStart;
    decltype(&AudioOutputUnitStop) stop = AudioOutputUnitStop;
    decltype(&AudioObjectGetPropertyDataSize) size = AudioObjectGetPropertyDataSize;
    decltype(&AudioObjectGetPropertyData) data = AudioObjectGetPropertyData;
    decltype(&AudioObjectAddPropertyListenerBlock) listen = AudioObjectAddPropertyListenerBlock;
    decltype(&AudioObjectRemovePropertyListenerBlock) unlisten = AudioObjectRemovePropertyListenerBlock;
};
#ifdef MIXYARD_OUTPUT_API_TEST
extern Api testApi;
#endif
} // namespace mixyard::output::detail
