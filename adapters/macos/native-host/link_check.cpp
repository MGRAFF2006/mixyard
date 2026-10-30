// SPDX-License-Identifier: MIT
// Force-load the production objects and resolve their actual SDK/Rust symbols.
// This executable must never run: SDK compilation/linkage is the only evidence.
int main() { return 2; }
