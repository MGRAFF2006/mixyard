// SPDX-License-Identifier: MIT
#pragma once
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#import <Foundation/Foundation.h>
#include <CoreAudio/CoreAudio.h>

namespace mixyard::macos::detail {
struct CaptureApi {
    decltype(&AudioHardwareCreateProcessTap) tap = AudioHardwareCreateProcessTap;
    decltype(&AudioHardwareDestroyProcessTap) untap = AudioHardwareDestroyProcessTap;
    decltype(&AudioHardwareCreateAggregateDevice) aggregate = AudioHardwareCreateAggregateDevice;
    decltype(&AudioHardwareDestroyAggregateDevice) unaggregate = AudioHardwareDestroyAggregateDevice;
    decltype(&AudioDeviceCreateIOProcID) create = AudioDeviceCreateIOProcID;
    decltype(&AudioDeviceDestroyIOProcID) destroy = AudioDeviceDestroyIOProcID;
    decltype(&AudioDeviceStart) start = AudioDeviceStart;
    decltype(&AudioDeviceStop) stop = AudioDeviceStop;
    decltype(&AudioObjectGetPropertyDataSize) size = AudioObjectGetPropertyDataSize;
    decltype(&AudioObjectGetPropertyData) data = AudioObjectGetPropertyData;
    decltype(&AudioObjectAddPropertyListenerBlock) listen = AudioObjectAddPropertyListenerBlock;
    decltype(&AudioObjectRemovePropertyListenerBlock) unlisten = AudioObjectRemovePropertyListenerBlock;
};
#ifdef MIXYARD_NATIVE_HOST_TEST
extern CaptureApi captureTestApi;
#endif
} // namespace mixyard::macos::detail
