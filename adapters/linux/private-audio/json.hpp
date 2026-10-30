// SPDX-License-Identifier: MIT
#pragma once
#include <algorithm>
#include <cctype>
#include <cstdint>
#include <cstring>
#include <json-c/json.h>
#include <memory>
#include <set>
#include <stdexcept>
#include <string>
#include <vector>

namespace wire {
constexpr size_t limit = 1024 * 1024;
struct Error : std::runtime_error {
  std::string code;
  Error(std::string c, std::string m)
      : std::runtime_error(m), code(std::move(c)) {}
};
inline void require(bool ok, const char *code, const char *why) {
  if (!ok)
    throw Error(code, why);
}
struct Delete {
  void operator()(json_object *p) const {
    if (p)
      json_object_put(p);
  }
};
using Json = std::unique_ptr<json_object, Delete>;
inline json_object *get(json_object *p, const char *key) {
  json_object *v = nullptr;
  require(p && json_object_object_get_ex(p, key, &v), "protocol-error",
          "missing key");
  return v;
}
inline std::string str(json_object *p) {
  require(p && json_object_is_type(p, json_type_string), "protocol-error",
          "expected string");
  const char *s = json_object_get_string(p);
  size_t n = json_object_get_string_len(p);
  require(s && n && n <= 4096 && strlen(s) == n, "protocol-error",
          "invalid string bound");
  return {s, n};
}
inline uint64_t num(json_object *p) {
  require(p && json_object_is_type(p, json_type_int), "protocol-error",
          "expected unsigned integer");
  const char *s = json_object_to_json_string_ext(p, JSON_C_TO_STRING_PLAIN);
  require(s[0] != '-', "protocol-error", "negative integer");
  return json_object_get_uint64(p);
}
inline void keys(json_object *p, std::initializer_list<const char *> names) {
  require(p && json_object_is_type(p, json_type_object), "protocol-error",
          "expected object");
  require((size_t)json_object_object_length(p) == names.size(),
          "protocol-error", "unknown or missing keys");
  for (auto k : names)
    (void)get(p, k);
}
inline bool hex(const std::string &s) {
  return s.size() == 64 && std::all_of(s.begin(), s.end(), [](char c) {
           return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
         });
}
inline std::string dump(json_object *p) {
  return json_object_to_json_string_ext(p, JSON_C_TO_STRING_PLAIN |
                                               JSON_C_TO_STRING_NOSLASHESCAPE);
}
inline json_object *object() { return json_object_new_object(); }
inline void add(json_object *p, const char *k, json_object *v) {
  json_object_object_add(p, k, v);
}
inline void add(json_object *p, const char *k, const std::string &v) {
  add(p, k, json_object_new_string_len(v.data(), (int)v.size()));
}
inline void add(json_object *p, const char *k, const char *v) {
  add(p, k, std::string(v));
}
inline void add(json_object *p, const char *k, uint64_t v) {
  add(p, k, json_object_new_uint64(v));
}
inline void boolean(json_object *p, const char *k, bool v) {
  add(p, k, json_object_new_boolean(v));
}
inline size_t count(json_object *p) {
  require(p && json_object_is_type(p, json_type_array), "protocol-error",
          "expected array");
  return json_object_array_length(p);
}
inline json_object *at(json_object *p, size_t i) {
  require(i < count(p), "protocol-error", "array bound");
  return json_object_array_get_idx(p, i);
}

/* json-c validates UTF-8/numbers/escapes; this first bounded scan additionally
 * rejects duplicate DECODED keys. It records exact Config bytes before any
 * reserialization, including float spelling, escaping and field order. */
struct Scan {
  const std::string &s;
  size_t i = 0, config_begin = 0, config_end = 0;
  explicit Scan(const std::string &text) : s(text) {}
  void space() {
    while (i < s.size() && std::isspace((unsigned char)s[i]))
      i++;
  }
  std::string string() {
    size_t start = i;
    require(i < s.size() && s[i++] == '"', "protocol-error",
            "expected JSON key");
    bool escape = false;
    while (i < s.size()) {
      char c = s[i++];
      if (escape) {
        escape = false;
        continue;
      }
      if (c == '\\') {
        escape = true;
        continue;
      }
      if (c == '"') {
        Json v(json_tokener_parse(s.substr(start, i - start).c_str()));
        return str(v.get());
      }
    }
    throw Error("protocol-error", "unterminated string");
  }
  void value(unsigned depth = 0, const std::string &path = "") {
    require(depth < 64, "protocol-error", "JSON nesting limit");
    space();
    require(i < s.size(), "protocol-error", "truncated JSON");
    if (s[i] == '{') {
      i++;
      space();
      std::set<std::string> seen;
      if (i < s.size() && s[i] == '}') {
        i++;
        return;
      }
      for (;;) {
        space();
        std::string key = string();
        require(seen.insert(key).second, "protocol-error",
                "duplicate JSON key");
        space();
        require(i < s.size() && s[i++] == ':', "protocol-error",
                "missing colon");
        space();
        size_t begin = i;
        std::string child = path + "/" + key;
        value(depth + 1, child);
        if (child == "/action/snapshot/config") {
          config_begin = begin;
          config_end = i;
        }
        space();
        require(i < s.size(), "protocol-error", "truncated object");
        char c = s[i++];
        if (c == '}')
          break;
        require(c == ',', "protocol-error", "invalid object");
      }
    } else if (s[i] == '[') {
      i++;
      space();
      if (i < s.size() && s[i] == ']') {
        i++;
        return;
      }
      for (;;) {
        value(depth + 1, path + "/*");
        space();
        require(i < s.size(), "protocol-error", "truncated array");
        char c = s[i++];
        if (c == ']')
          break;
        require(c == ',', "protocol-error", "invalid array");
      }
    } else if (s[i] == '"')
      (void)string();
    else {
      size_t start = i;
      while (i < s.size() && s[i] != ',' && s[i] != ']' && s[i] != '}' &&
             !std::isspace((unsigned char)s[i]))
        i++;
      require(i > start, "protocol-error", "missing value");
    }
  }
};
inline Json parse(const std::string &s, std::string *config = nullptr,
                  size_t bound = limit) {
  require(!s.empty() && s.size() <= bound && s.find('\0') == std::string::npos,
          "protocol-error", "JSON byte bound");
  Scan scan(s);
  scan.value();
  scan.space();
  require(scan.i == s.size(), "protocol-error", "trailing JSON");
  std::unique_ptr<json_tokener, decltype(&json_tokener_free)> t(
      json_tokener_new_ex(64), json_tokener_free);
  json_tokener_set_flags(t.get(),
                         JSON_TOKENER_STRICT | JSON_TOKENER_VALIDATE_UTF8);
  Json result(json_tokener_parse_ex(t.get(), s.data(), (int)s.size()));
  require(json_tokener_get_error(t.get()) == json_tokener_success && result,
          "protocol-error", "invalid JSON");
  if (config && scan.config_end > scan.config_begin)
    *config = s.substr(scan.config_begin, scan.config_end - scan.config_begin);
  return result;
}
} // namespace wire
