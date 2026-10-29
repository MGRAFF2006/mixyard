/* SPDX-License-Identifier: MIT */
#include "../worker.h"
_Static_assert(_Generic(&myw_port_scrub_quiescent, int (*)(myw_port*): 1, default: 0),
               "quiescent scrub must retain its C11 ABI");
