cmake_minimum_required(VERSION 3.20)
include("${CMAKE_CURRENT_LIST_DIR}/native_link.cmake")
# Exact native-static-libs line from the failing hosted Windows toolchain.
set(tokens kernel32.lib ntdll.lib userenv.lib ws2_32.lib dbghelp.lib /defaultlib:msvcrt)
mixyard_msvc_native_link("${tokens}" libraries options)
if(NOT libraries STREQUAL "kernel32.lib;ntdll.lib;userenv.lib;ws2_32.lib;dbghelp.lib" OR
   NOT options STREQUAL "/defaultlib:msvcrt")
  message(FATAL_ERROR "Rust native libraries/directives changed or became filenames")
endif()
message(STATUS "PASS actual MSVC native-static-libs regression: /defaultlib stays a linker option")
