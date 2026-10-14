// SPDX-License-Identifier: MIT
#pragma once
#include "../driver/ioctl.hpp"
#include "status.hpp"
namespace mixyard_input {
constexpr uint32_t ioctl_capture_status = CTL_CODE(device_type, 0x802, METHOD_BUFFERED,
    FILE_READ_ACCESS | FILE_WRITE_ACCESS);
}
