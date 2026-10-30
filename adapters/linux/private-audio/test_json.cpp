// SPDX-License-Identifier: MIT
#include "json.hpp"
#include "sha256.h"
#include <cassert>
#include <iostream>
static bool rejected(const std::string &s) {
  try {
    (void)wire::parse(s);
    return false;
  } catch (const wire::Error &) {
    return true;
  }
}
int main() {
  assert(rejected("{\"action\":1,\"action\":2}"));
  assert(rejected("{\"action\":1,\"\\u0061ction\":2}"));
  assert(rejected("{\"a\":1} {\"a\":2}"));
  assert(rejected("{\"a\":NaN}"));
  assert(rejected("{\"a\":Infinity}"));
  assert(rejected("{\"a\":-Infinity}"));
  assert(rejected("{\"a\":[{\"b\":NaN}]}"));
  assert(rejected("{\"a\":1e999}"));
  auto finite = wire::parse(
      "{\"text\":\"NaN/Infinity\",\"values\":[0,-0.25,1e20,true,null]}");
  assert(wire::str(wire::get(finite.get(), "text")) == "NaN/Infinity");
  assert(rejected(std::string(wire::limit + 1, ' ')));
  constexpr char embedded_nul[] = "{\"a\":\"x\0y\"}";
  assert(rejected(std::string(embedded_nul, sizeof(embedded_nul) - 1)));
  std::string raw;
  auto j =
      wire::parse("{\"action\":{\"kind\":\"prepare\",\"snapshot\":{\"config\":{"
                  "\"revision\":29,\"gain\":1.0,\"label\":\"A/B\"}}}}",
                  &raw);
  assert(raw == "{\"revision\":29,\"gain\":1.0,\"label\":\"A/B\"}");
  char a[65], b[65];
  mx_sha_hex(raw.data(), raw.size(), a);
  std::string changed = "{\"revision\":29,\"gain\":1,\"label\":\"A/B\"}";
  mx_sha_hex(changed.data(), changed.size(), b);
  assert(strcmp(a, b));
  bool failed = false;
  try {
    wire::keys(j.get(), {"action", "alias"});
  } catch (const wire::Error &) {
    failed = true;
  }
  assert(failed);
  std::cout << "strict duplicate/unknown/size/JSON rejection and exact Config "
               "bytes: OK\n";
}
