// SPDX-License-Identifier: MIT
// Persistent owned-private-generated-v1 client. No physical/default server
// path.
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include "json.hpp"
#include "owner.h"
#include "control_transport.h"
#include "sha256.h"
#include <array>
#include <atomic>
#include <cerrno>
#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <dlfcn.h>
#include <fcntl.h>
#include <fstream>
#include <limits.h>
#include <map>
#include <pipewire/data-loop.h>
#include <pipewire/filter.h>
#include <pipewire/pipewire.h>
#include <pthread.h>
#include <set>
#include <spa/buffer/meta.h>
#include <spa/param/audio/format-utils.h>
#include <sys/stat.h>
#include <unistd.h>

using namespace wire;
constexpr unsigned frames = 128, stride = 16, bytes = frames * stride,
                   pool_slots = 8;
static_assert(std::atomic<uint64_t>::is_always_lock_free,
              "RT requires lock-free scalar publication");
static_assert(std::atomic<bool>::is_always_lock_free,
              "RT requires lock-free gates");
static_assert(__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__,
              "this first exact native format requires little-endian f64");
struct App;
struct Generation;
struct EndpointNode;
struct EndpointId {
  uint32_t node = SPA_ID_INVALID, port = SPA_ID_INVALID;
  uint64_t node_serial = 0, port_serial = 0;
};
static bool equal(const EndpointId &a, const EndpointId &b) {
  return a.node == b.node && a.port == b.port &&
         a.node_serial == b.node_serial && a.port_serial == b.port_serial;
}
static json_object *endpoint_json(const EndpointId &e) {
  auto j = object();
  add(j, "nodeId", (uint64_t)e.node);
  add(j, "nodeSerial", e.node_serial);
  add(j, "portId", (uint64_t)e.port);
  add(j, "portSerial", e.port_serial);
  return j;
}
static EndpointId endpoint_parse(json_object *j) {
  keys(j, {"nodeId", "nodeSerial", "portId", "portSerial"});
  uint64_t n = num(get(j, "nodeId")), p = num(get(j, "portId"));
  require(n < SPA_ID_INVALID && p < SPA_ID_INVALID, "endpoint-changed",
          "endpoint ID bound");
  EndpointId e{(uint32_t)n, (uint32_t)p, num(get(j, "nodeSerial")),
               num(get(j, "portSerial"))};
  require(e.node_serial && e.port_serial, "endpoint-changed", "missing serial");
  return e;
}
static json_object *format_json() {
  auto j = object();
  add(j, "encoding", "f64-le");
  add(j, "sampleRateHz", uint64_t(48000));
  auto c = json_object_new_array();
  json_object_array_add(c, json_object_new_string("FL"));
  json_object_array_add(c, json_object_new_string("FR"));
  add(j, "channels", c);
  add(j, "frames", uint64_t(frames));
  return j;
}
static std::string digest(const std::string &s) {
  char h[65];
  mx_sha_hex(s.data(), s.size(), h);
  return h;
}
static std::string read_file(const std::string &path,
                             size_t bound = wire::limit) {
  struct stat st;
  require(!lstat(path.c_str(), &st) && S_ISREG(st.st_mode) && st.st_size > 0 &&
              (uint64_t)st.st_size <= bound,
          "protocol-error", "bounded regular file required");
  int fd = open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
  require(fd >= 0, "protocol-error", "file open failed");
  std::string s((size_t)st.st_size, '\0');
  size_t n = 0;
  while (n < s.size()) {
    ssize_t r = read(fd, s.data() + n, s.size() - n);
    if (r < 0 && errno == EINTR)
      continue;
    if (r <= 0) {
      close(fd);
      throw Error("protocol-error", "file read failed");
    }
    n += (size_t)r;
  }
  char extra;
  ssize_t r = read(fd, &extra, 1);
  close(fd);
  require(r == 0, "protocol-error", "file changed during read");
  return s;
}
static std::string absolute(const std::string &p) {
  char b[PATH_MAX];
  require(!p.empty() && p[0] == '/' && realpath(p.c_str(), b),
          "unqualified-sdk", "absolute existing path required");
  return b;
}
static bool contained(const std::string &p, const std::string &prefix) {
  return p.size() > prefix.size() && p.compare(0, prefix.size(), prefix) == 0 &&
         p[prefix.size()] == '/';
}
static uint64_t decimal(const char *s) {
  require(s && *s && *s != '-', "endpoint-changed",
          "missing numeric native property");
  errno = 0;
  char *end = nullptr;
  auto n = strtoull(s, &end, 10);
  require(!errno && end && !*end, "endpoint-changed",
          "invalid numeric native property");
  return n;
}
static void private_dir(const char *p) {
  struct stat st;
  require(p && p[0] == '/' && !lstat(p, &st) && S_ISDIR(st.st_mode) &&
              st.st_uid == getuid() && (st.st_mode & 0777) == 0700,
          "unsupported-private-target", "owned 0700 directory required");
}

struct BufferRecord {
  std::atomic<uintptr_t> token{0};
  std::atomic<uint64_t> zero_epoch{0};
};
struct Port {
  EndpointNode *node = nullptr;
  unsigned index = 0;
  bool output = false;
  std::atomic<bool> format_ok{false};
  std::array<BufferRecord, pool_slots> buffers;
};
struct Source {
  std::string id;
  json_object *config = nullptr;
  double left = 0, right = 0;
  uint64_t marker = 0, limit = 0, generated = 0;
  std::array<double, 2 * frames> pcm{};
};
struct Output {
  std::string id, mix, label, link_op;
  json_object *config = nullptr;
  EndpointId published, consumer;
  bool bound = false, link_replied = false;
  std::atomic<bool> closed{false};
  std::atomic<uint64_t> first_commit{UINT64_MAX};
};
struct EndpointNode {
  Generation *generation = nullptr;
  unsigned role = 0, index = 0; // generator, engine, output bus
  pw_filter *filter = nullptr;
  spa_hook hook{};
  std::vector<Port *> in, out;
  std::atomic<uint64_t> clock{0};
};
struct LinkRecord {
  pw_proxy *proxy = nullptr;
  spa_hook hook{};
  std::atomic<bool> active{false};
  bool format_ok = false;
  EndpointId from, to;
};
struct Generation {
  App *app = nullptr;
  uint64_t generation = 0, epoch = 0, store = 0, revision = 0,
           admitted_inventory = 0, clock = 0, budget = 0;
  std::atomic<uint64_t> inventory{0};
  mx_private_stop_tuple pending_target{};
  size_t ns = 0, no = 0, nm = 0, ports = 0;
  Json config;
  std::unique_ptr<Source[]> sources;
  std::unique_ptr<Output[]> outputs;
  std::unique_ptr<mx_input[]> inputs;
  std::unique_ptr<EndpointNode[]> nodes;
  std::unique_ptr<LinkRecord[]> links;
  size_t link_count = 0;
  std::string prepared, staged, prepare_op, stage_op, install_op, request,
      request_digest, endpoint_digest;
  mx_host *host = nullptr, *staged_host = nullptr;
  mx_private_timeline timeline{};
  std::atomic<bool> sealed{false}, warming{false}, headers{false},
      switched{false}, retired{false}, complete{false};
  std::atomic<unsigned> fault{0};
  std::atomic<bool> scrub_failed{false};
  std::atomic<uint64_t> seen_clock{0}, seen_frame{0}, seen_ns{0},
      switch_frame{UINT64_MAX}, first_processed{UINT64_MAX};
  std::atomic<uint64_t> generator_clock{0};
  bool prepare_replied = false, stage_started = false, install_replied = false,
       objects_destroyed = false;
  unsigned warm_ticks = 0;
};
struct Global {
  App *app = nullptr;
  uint32_t id = SPA_ID_INVALID, node = SPA_ID_INVALID;
  uint64_t serial = 0;
  bool is_node = false, is_port = false, alive = false, input = false,
       format_ok = false;
  std::string name;
  pw_proxy *proxy = nullptr;
  spa_hook hook{};
};
struct InputLine {
  std::string data;
  bool closed = false;
};
struct Build {
  pthread_t thread{};
  bool started = false;
  std::atomic<bool> done{false};
  Generation *g = nullptr;
  mx_host *host = nullptr;
  int result = MX_INVALID;
  char error[512]{};
};
struct App {
  std::string nonce, prefix;
  Json descriptor;
  uint64_t max_sources = 0, max_outputs = 0, max_bytes = 0, resident = 0,
           last_generation = 0, inventory = 1, published_generation = 0;
  std::atomic<uint64_t> epoch{1};
  std::atomic<bool> admission{true};
  std::atomic<unsigned> fault{0};
  pw_main_loop *loop = nullptr;
  pw_context *context = nullptr;
  pw_data_loop *owner = nullptr;
  pw_core *core = nullptr;
  pw_registry *registry = nullptr;
  spa_hook core_hook{}, registry_hook{};
  spa_source *timer = nullptr;
  int cancel_fd = -1;
  InputLine normal, cancel;
  std::deque<std::string> replies;
  size_t reply_offset = 0, queued_bytes = 0;
  std::vector<std::unique_ptr<Global>> globals;
  size_t global_budget = 0;
  unsigned modules = 0;
  std::array<uint32_t, 6> module_ids{{SPA_ID_INVALID, SPA_ID_INVALID,
                                      SPA_ID_INVALID, SPA_ID_INVALID,
                                      SPA_ID_INVALID, SPA_ID_INVALID}};
  bool core_ok = false, ready = false, closing = false, failed_held = false;
  std::vector<std::unique_ptr<Generation>> generations;
  Generation *candidate = nullptr;
  Generation *active = nullptr; // owner ONLY; control reads publication below
  std::atomic<uint64_t> active_generation{0};
  mx_private_reservation control{}; // received priority reservation, control-only
  bool fence_pending = false;
  Generation *ticket = nullptr; // retained immutable until owner retirement
  std::atomic<Generation *> pending{nullptr};
  Build build;
  bool fencing = false, fence_owner_queued = false, fence_replied = false,
       cleanup_queued = false;
  std::atomic<bool> fence_owner_done{false}, cleanup_done{false};
  std::string fence_op, shutdown_op;
  uint64_t fence_epoch = 0;
  Generation *fence_ticket = nullptr;
  Generation *settled_fence_ticket = nullptr;
  std::string phase = "idle", last_code, last_message;
  std::set<std::string> operations;
  bool exit_ready = false, transport_failed = false,
       native_fault_reported = false;
};
static void pump(App *a, bool priority);
static bool continue_build(App *a) {
  pump(a, true);
  return a->admission.load(std::memory_order_acquire) && !a->fencing &&
         !a->closing;
}

static void rt_fault(Generation *g, unsigned code) {
  unsigned zero = 0;
  g->fault.compare_exchange_strong(zero, code, std::memory_order_release);
  g->headers.store(false, std::memory_order_release);
}
static bool current(Generation *g) {
  return g->app->admission.load(std::memory_order_acquire) &&
         g->epoch == g->app->epoch.load(std::memory_order_acquire) &&
         !g->retired.load(std::memory_order_acquire) &&
         !g->fault.load(std::memory_order_acquire) &&
         !g->app->fault.load(std::memory_order_acquire);
}
static BufferRecord *record(Port *p, pw_buffer *b) {
  uintptr_t key = (uintptr_t)b;
  for (auto &r : p->buffers)
    if (r.token.load(std::memory_order_acquire) == key)
      return &r;
  return nullptr;
}
static spa_meta_header *header(pw_buffer *b) {
  return b && b->buffer
             ? (spa_meta_header *)spa_buffer_find_meta_data(
                   b->buffer, SPA_META_Header, sizeof(spa_meta_header))
             : nullptr;
}
static double *buffer_data(Port *p, pw_buffer *b, bool input) {
  if (!b || !b->buffer || b->buffer->n_datas != 1 || !b->buffer->datas ||
      !record(p, b))
    return nullptr;
  spa_data *d = &b->buffer->datas[0];
  if (!d->data || !d->chunk || d->maxsize != bytes || !header(b))
    return nullptr;
  if ((uintptr_t)d->data % alignof(double))
    return nullptr;
  if (input && (d->chunk->offset || d->chunk->size != bytes ||
                d->chunk->stride != (int)stride || d->chunk->flags))
    return nullptr;
  return (double *)d->data;
}
static mx_private_input_view input_view(pw_buffer *b) {
  mx_private_input_view v{};
  if (!b || !b->buffer)
    return v;
  v.n_datas = b->buffer->n_datas;
  if (v.n_datas != 1 || !b->buffer->datas)
    return v;
  auto d = &b->buffer->datas[0];
  v.data = d->data;
  v.capacity = d->maxsize;
  if (d->chunk) {
    v.chunk = true;
    v.offset = d->chunk->offset;
    v.size = d->chunk->size;
    v.stride = d->chunk->stride;
    v.chunk_flags = d->chunk->flags;
  }
  auto h = header(b);
  if (h) {
    v.header = true;
    v.pts = h->pts;
    v.sequence = h->seq;
    v.header_flags = h->flags;
    v.header_offset = h->offset;
    v.dts_offset = h->dts_offset;
  }
  return v;
}
static bool fresh(pw_buffer *b, const spa_io_position *pos,
                  bool allow_gap = false) {
  auto h = header(b);
  return h && h->seq == pos->clock.position &&
         h->pts == (int64_t)pos->clock.nsec && !h->offset && !h->dts_offset &&
         (!h->flags || (allow_gap && h->flags == SPA_META_HEADER_FLAG_GAP));
}
static bool return_buffer(Port *p, pw_buffer *b) {
  bool returned = pw_filter_queue_buffer(p, b) >= 0;
  if (!returned)
    p->node->generation->scrub_failed.store(true, std::memory_order_release);
  return returned;
}
static void discard_output(Port *p, pw_buffer *b) {
  // A malformed output can never be queued with its previous valid-data size.
  // Zero payload only where this live registered borrow proves its fixed pool.
  if (b && b->buffer && b->buffer->n_datas == 1 && b->buffer->datas) {
    auto d = &b->buffer->datas[0];
    if (record(p, b) && d->data && d->maxsize == bytes)
      memset(d->data, 0, bytes);
    if (d->chunk) {
      d->chunk->offset = 0;
      d->chunk->size = 0;
      d->chunk->stride = stride;
      d->chunk->flags = SPA_CHUNK_FLAG_EMPTY;
    }
    auto h = header(b);
    if (h) {
      *h = spa_meta_header{};
      h->flags = SPA_META_HEADER_FLAG_GAP;
    }
  }
  p->node->generation->scrub_failed.store(true, std::memory_order_release);
  (void)return_buffer(p, b);
}
struct InputBorrow {
  Port *port;
  pw_buffer *buffer;
};
static bool return_input_borrow(void *data) {
  auto borrow = (InputBorrow *)data;
  return return_buffer(borrow->port, borrow->buffer);
}
static bool submit(Port *p, pw_buffer *b, const spa_io_position *pos,
                   bool silent) {
  auto d = &b->buffer->datas[0];
  d->chunk->offset = 0;
  d->chunk->stride = stride;
  d->chunk->size = bytes;
  d->chunk->flags = 0;
  *header(b) = spa_meta_header{};
  auto h = header(b);
  h->seq = pos->clock.position;
  h->pts = (int64_t)pos->clock.nsec;
  h->flags = silent ? SPA_META_HEADER_FLAG_GAP : 0;
  b->size = frames;
  return return_buffer(p, b);
}
/* Scrub only LIVE borrows, including metadata/chunks. Token addresses are
 * opaque registration identities; no native pointer is dereferenced later. */
static void scrub_port(Port *p, const spa_io_position *pos, uint64_t epoch) {
  for (unsigned i = 0; i < pool_slots; i++) {
    pw_buffer *b = pw_filter_dequeue_buffer(p);
    if (!b)
      break;
    auto r = record(p, b);
    double *pcm = buffer_data(p, b, false);
    if (pcm && r) {
      auto d = &b->buffer->datas[0];
      memset(d->data, 0, d->maxsize);
      r->zero_epoch.store(epoch, std::memory_order_release);
      if (!submit(p, b, pos, true)) {
        rt_fault(p->node->generation, 3);
        p->node->generation->scrub_failed.store(true,
                                                std::memory_order_release);
      }
    } else {
      rt_fault(p->node->generation, 4);
      p->node->generation->scrub_failed.store(true, std::memory_order_release);
      discard_output(p, b);
    }
  }
}
static void revoke(Generation *g) {
  if (!g || g->retired.load(std::memory_order_acquire))
    return;
  if (g->host)
    mx_host_revoke(g->host);
  for (size_t s = 0; s < g->ns; s++) {
    g->sources[s].pcm.fill(0);
    g->sources[s].generated = 0;
    g->inputs[s] = mx_input{};
  }
  g->timeline = mx_private_timeline{};
  g->headers.store(false, std::memory_order_release);
  g->retired.store(true, std::memory_order_release);
}
static bool graph_formats(Generation *g) {
  for (size_t n = 0; n < 2 + g->no; n++) {
    auto &e = g->nodes[n];
    for (auto p : e.in)
      if (!p->format_ok.load(std::memory_order_acquire))
        return false;
    for (auto p : e.out)
      if (!p->format_ok.load(std::memory_order_acquire))
        return false;
  }
  return true;
}
static bool source_formats(Generation *g) {
  for (auto p : g->nodes[0].out)
    if (!p->format_ok.load(std::memory_order_acquire)) return false;
  for (auto p : g->nodes[1].in)
    if (!p->format_ok.load(std::memory_order_acquire)) return false;
  return true;
}
static bool graph_clock(Generation *g, uint64_t clock) {
  for (size_t n = 0; n < 2 + g->no; n++)
    if (g->nodes[n].clock.load(std::memory_order_acquire) != clock)
      return false;
  return true;
}
static void process(void *userdata, spa_io_position *pos) {
  auto e = (EndpointNode *)userdata;
  auto g = e->generation;
  auto a = g->app;
  if (!pw_data_loop_in_thread(a->owner)) {
    rt_fault(g, 1);
    g->scrub_failed.store(true, std::memory_order_release);
    return;
  }
  if (!g->sealed.load(std::memory_order_acquire))
    return;
  if (!pos || pos->clock.duration != frames || pos->clock.rate.num != 1 ||
      pos->clock.rate.denom != 48000 || !pos->clock.id ||
      pos->clock.id == SPA_ID_INVALID ||
      pos->clock.nsec > INT64_MAX - 100000000 ||
      pos->clock.position > UINT64_MAX - frames) {
    rt_fault(g, 2);
    revoke(g);
    return;
  }
  e->clock.store(pos->clock.id, std::memory_order_release);
  if (!current(g)) {
    if (!g->retired.load(std::memory_order_acquire))
      revoke(g);
    for (auto p : e->in)
      scrub_port(p, pos, a->epoch.load(std::memory_order_acquire));
    for (auto p : e->out)
      scrub_port(p, pos, a->epoch.load(std::memory_order_acquire));
    return;
  }
  if (e->role == 0) {
    g->generator_clock.store(pos->clock.id, std::memory_order_release);
    for (size_t s = 0; s < g->ns; s++) {
      auto &source = g->sources[s];
      auto p = e->out[s];
      auto b = pw_filter_dequeue_buffer(p);
      if (!b) {
        if (g->switched.load(std::memory_order_acquire))
          rt_fault(g, 5);
        continue;
      }
      double *pcm = buffer_data(p, b, false);
      bool eof = source.limit && (source.generated > source.limit ||
                                  source.limit - source.generated < frames);
      if (!pcm) {
        rt_fault(g, 6);
        discard_output(p, b);
        continue;
      }
      if (eof || !current(g)) {
        memset(pcm, 0, bytes);
        rt_fault(g, 7);
        (void)submit(p, b, pos, true);
        continue;
      }
      for (unsigned f = 0; f < frames; f++) {
        bool marker =
            source.marker && ((source.generated + f) % source.marker == 0);
        pcm[2 * f] = marker ? 0.0 : source.left;
        pcm[2 * f + 1] = marker ? -0.0 : source.right;
      }
      if (source.generated > UINT64_MAX - frames) {
        memset(pcm, 0, bytes);
        rt_fault(g, 8);
      } else
        source.generated += frames;
      if (!current(g))
        memset(pcm, 0, bytes);
      if (!submit(p, b, pos, !current(g)))
        rt_fault(g, 9);
    }
    if (!current(g))
      revoke(g);
    return;
  }
  if (e->role == 2) {
    auto &out = g->outputs[e->index];
    auto in = e->in[0], dst = e->out[0];
    auto b = pw_filter_dequeue_buffer(in);
    auto target = pw_filter_dequeue_buffer(dst);
    bool committed = false;
    bool running =
        g->switched.load(std::memory_order_acquire) && a->active == g;
    double *src = b ? buffer_data(in, b, true) : nullptr,
           *pcm = target ? buffer_data(dst, target, false) : nullptr;
    bool valid =
        src &&
        fresh(b, pos, !running || out.closed.load(std::memory_order_acquire));
    bool silent = !running || out.closed.load(std::memory_order_acquire) ||
                  !valid || !current(g);
    if (running && (!valid || !pcm))
      out.closed.store(true, std::memory_order_release); // destination-local
    if (pcm) {
      if (silent)
        memset(pcm, 0, bytes);
      else
        memcpy(pcm, src, bytes);
    }
    if (b && !return_buffer(in, b))
      rt_fault(g, 10);
    // This second gate is at the actual native output commit, after returning
    // the input token. The fence barrier serializes after this owner turn.
    if (target) {
      if (pcm) {
        if (!current(g)) {
          memset(pcm, 0, bytes);
          silent = true;
        }
        committed = submit(dst, target, pos, silent) && !silent;
      } else
        discard_output(dst, target);
    }
    if (committed) {
      uint64_t unset = UINT64_MAX;
      out.first_commit.compare_exchange_strong(unset, pos->clock.position,
                                               std::memory_order_release);
    }
    return;
  }
  bool ready =
      g->warming.load(std::memory_order_acquire) && source_formats(g) &&
      g->generator_clock.load(std::memory_order_acquire) == pos->clock.id;
  for (size_t s = 0; s < g->ns; s++) {
    auto p = e->in[s];
    auto b = pw_filter_dequeue_buffer(p);
    auto &source = g->sources[s];
    bool valid = false;
    if (b) {
      auto view = input_view(b);
      if (!record(p, b))
        view.data = nullptr;
      InputBorrow borrow{p, b};
      auto result = mx_private_copy_and_return(
          &view, g->switched.load(std::memory_order_acquire),
          pos->clock.position, pos->clock.nsec, SPA_CHUNK_FLAG_EMPTY,
          source.pcm.data(), return_input_borrow, &borrow);
      valid = result == MX_PRIVATE_INPUT_READY;
      if (result == MX_PRIVATE_INPUT_FATAL)
        rt_fault(g, 26);
    }
    ready = ready && valid;
    g->inputs[s] = mx_input{
        (const uint8_t *)source.pcm.data(), bytes, MX_F64LE, 48000, 2, {}};
  }
  g->seen_frame.store(pos->clock.position, std::memory_order_relaxed);
  g->seen_ns.store(pos->clock.nsec, std::memory_order_relaxed);
  g->seen_clock.store(pos->clock.id, std::memory_order_release);
  g->headers.store(ready, std::memory_order_release);
  auto pending = a->pending.load(std::memory_order_acquire);
  if (pending == g && !g->switched.load(std::memory_order_acquire) && ready &&
      pos->clock.id == g->clock && graph_clock(g, g->clock) && current(g) &&
      g->inventory.load(std::memory_order_acquire) == g->admitted_inventory &&
      mx_private_ticket_current(g->epoch,
                                a->epoch.load(std::memory_order_acquire),
                                a->admission.load(std::memory_order_acquire),
                                g->generation, g->generation)) {
    if (a->active && a->active != g)
      revoke(a->active);
    a->active = g;
    g->host = g->staged_host;
    g->timeline = mx_private_timeline{
        pos->clock.id, g->generation, g->revision, 0, 0, 0, 0, false};
    g->switch_frame.store(pos->clock.position, std::memory_order_relaxed);
    g->switched.store(true, std::memory_order_release);
    a->active_generation.store(g->generation, std::memory_order_release);
    // Pending remains exact until control has serialized a complete receipt
    // or a priority fence has retired this retained ticket.
  }
  bool running = a->active == g && g->switched.load(std::memory_order_acquire);
  bool rendered = false;
  if (running && (!ready || !current(g))) {
    rt_fault(g, 12);
    revoke(g);
  }
  if (running && ready && current(g)) {
    mx_stamp stamp{};
    if (!mx_private_stamp(&g->timeline, pos->clock.id, pos->clock.position,
                          pos->clock.nsec, &stamp)) {
      rt_fault(g, 13);
      revoke(g);
    } else {
      for (size_t s = 0; s < g->ns; s++)
        g->inputs[s].stamp = stamp;
      if (mx_host_process(g->host, pos->clock.nsec, stamp, frames,
                          g->inputs.get(), g->ns) == MX_OK) {
        rendered = true;
        uint64_t native_processed = 0;
        if (!mx_private_processed_native(&g->timeline, &stamp,
                                         pos->clock.position, &native_processed)) {
          rt_fault(g, 13); revoke(g); rendered = false;
        } else {
          uint64_t unset = UINT64_MAX;
          g->first_processed.compare_exchange_strong(unset, native_processed,
                                                     std::memory_order_release);
        }
      } else {
        rt_fault(g, 14);
        revoke(g);
      }
    }
  }
  for (size_t o = 0; o < g->no; o++) {
    auto p = e->out[o];
    if (!p->format_ok.load(std::memory_order_acquire)) {
      g->outputs[o].closed.store(true, std::memory_order_release);
    }
    auto b = pw_filter_dequeue_buffer(p);
    if (!b) {
      if (running)
        g->outputs[o].closed.store(true, std::memory_order_release);
      continue;
    }
    auto pcm = buffer_data(p, b, false);
    bool silent = true;
    if (pcm) {
      const double *borrowed = nullptr;
      size_t n = 0;
      int result = rendered && current(g)
                       ? mx_host_output(g->host, o, &borrowed, &n)
                       : MX_INACTIVE;
      if (result == MX_OK && borrowed && n == 2 * frames &&
          !g->outputs[o].closed.load(std::memory_order_acquire)) {
        memcpy(pcm, borrowed, bytes);
        silent = false;
      } else {
        memset(pcm, 0, bytes);
        if (running && result != MX_INACTIVE)
          g->outputs[o].closed.store(true, std::memory_order_release);
      }
      // No borrowed host output survives this expression/callback.
      if (!current(g)) {
        memset(pcm, 0, bytes);
        silent = true;
      }
      if (!submit(p, b, pos, silent))
        g->outputs[o].closed.store(true, std::memory_order_release);
    } else {
      g->outputs[o].closed.store(true, std::memory_order_release);
      discard_output(p, b);
    }
  }
  if (running && !current(g))
    revoke(g);
  if (running && current(g) &&
      g->first_processed.load(std::memory_order_acquire) != UINT64_MAX) {
    bool all = true;
    for (size_t o = 0; o < g->no; o++)
      if (g->outputs[o].first_commit.load(std::memory_order_acquire) ==
          UINT64_MAX)
        all = false;
    if (all)
      g->complete.store(
          true,
          std::memory_order_release); // actual exclusive owner terminal barrier
  }
}

static void add_buffer(void *data, void *port_data, pw_buffer *b) {
  auto e = (EndpointNode *)data;
  auto p = (Port *)port_data;
  if (!p || !b) {
    rt_fault(e->generation, 15);
    return;
  }
  for (auto &r : p->buffers) {
    uintptr_t zero = 0;
    if (r.token.compare_exchange_strong(zero, (uintptr_t)b,
                                        std::memory_order_acq_rel)) {
      r.zero_epoch.store(0, std::memory_order_release);
      return;
    }
  }
  rt_fault(e->generation, 16);
  e->generation->scrub_failed.store(true, std::memory_order_release);
}
static void remove_buffer(void *data, void *port_data, pw_buffer *b) {
  auto e = (EndpointNode *)data;
  auto p = (Port *)port_data;
  auto r = p ? record(p, b) : nullptr;
  if (r) {
    if (e->generation->epoch !=
            e->generation->app->epoch.load(std::memory_order_acquire) &&
        r->zero_epoch.load(std::memory_order_acquire) <
            e->generation->app->epoch.load(std::memory_order_acquire))
      e->generation->scrub_failed.store(true, std::memory_order_release);
    r->token.store(0, std::memory_order_release);
  } else {
    rt_fault(e->generation, 17);
    e->generation->scrub_failed.store(true, std::memory_order_release);
  }
}
static bool exact_format(const spa_pod *param) {
  spa_audio_info_raw f{};
  return param && spa_format_audio_raw_parse(param, &f) >= 0 &&
         f.format == SPA_AUDIO_FORMAT_F64_LE && f.rate == 48000 &&
         f.channels == 2 && !f.flags && f.position[0] == SPA_AUDIO_CHANNEL_FL &&
         f.position[1] == SPA_AUDIO_CHANNEL_FR;
}
static void param_changed(void *data, void *port_data, uint32_t id,
                          const spa_pod *param) {
  if (id != SPA_PARAM_Format || !port_data)
    return;
  auto e = (EndpointNode *)data;
  auto p = (Port *)port_data;
  bool ok = exact_format(param);
  p->format_ok.store(ok, std::memory_order_release);
  auto scope = mx_private_format_scope(e->role, p->output, ok, param != nullptr,
      e->generation->switched.load(std::memory_order_acquire));
  if (scope == MX_PRIVATE_FORMAT_DESTINATION) {
    auto index = e->role == 2 ? e->index : p->index;
    e->generation->outputs[index].closed.store(true, std::memory_order_release);
  } else if (scope == MX_PRIVATE_FORMAT_SOURCE) {
    rt_fault(e->generation, param ? 19 : 18);
  }
}
static void filter_state(void *data, pw_filter_state old, pw_filter_state state,
                         const char *error) {
  (void)old;
  (void)error;
  if (state == PW_FILTER_STATE_ERROR) {
    auto e = (EndpointNode *)data;
    rt_fault(e->generation, 20);
  }
}
static const pw_filter_events filter_events = {.version =
                                                   PW_VERSION_FILTER_EVENTS,
                                               .state_changed = filter_state,
                                               .param_changed = param_changed,
                                               .add_buffer = add_buffer,
                                               .remove_buffer = remove_buffer,
                                               .process = process};

static Port *add_port(EndpointNode *e, bool output, unsigned index) {
  uint8_t storage[2048];
  spa_pod_builder b = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
  spa_audio_info_raw f{};
  f.format = SPA_AUDIO_FORMAT_F64_LE;
  f.rate = 48000;
  f.channels = 2;
  f.position[0] = SPA_AUDIO_CHANNEL_FL;
  f.position[1] = SPA_AUDIO_CHANNEL_FR;
  const spa_pod *params[3];
  params[0] = spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &f);
  params[1] = (const spa_pod *)spa_pod_builder_add_object(
      &b, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
      SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, pool_slots),
      SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1), SPA_PARAM_BUFFERS_size,
      SPA_POD_Int(bytes), SPA_PARAM_BUFFERS_stride, SPA_POD_Int(stride));
  params[2] = (const spa_pod *)spa_pod_builder_add_object(
      &b, SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta, SPA_PARAM_META_type,
      SPA_POD_Id(SPA_META_Header), SPA_PARAM_META_size,
      SPA_POD_Int(sizeof(spa_meta_header)));
  std::string name = (output ? "out-" : "in-") + std::to_string(index);
  void *mem = pw_filter_add_port(
      e->filter, output ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT,
      PW_FILTER_PORT_FLAG_MAP_BUFFERS, sizeof(Port),
      pw_properties_new(PW_KEY_PORT_NAME, name.c_str(), nullptr), params, 3);
  require(mem, "owner-failed", "native port creation failed");
  auto p = new (mem) Port;
  p->node = e;
  p->output = output;
  p->index = index;
  return p;
}
static void create_graph(Generation *g) {
  auto a = g->app;
  g->nodes = std::make_unique<EndpointNode[]>(2 + g->no);
  g->links = std::make_unique<LinkRecord[]>(g->ns + 2 * g->no);
  for (size_t n = 0; n < 2 + g->no; n++) {
    if (!continue_build(a))
      return;
    auto e = &g->nodes[n];
    e->generation = g;
    e->role = n < 2 ? (unsigned)n : 2;
    e->index = n < 2 ? 0 : (unsigned)(n - 2);
    std::string name = "mixyard-private-" + a->nonce.substr(0, 16) + "-" +
                       std::to_string(g->generation) + "-" +
                       (n == 0   ? "generator"
                        : n == 1 ? "engine"
                                 : "bus-" + std::to_string(n - 2));
    auto props = pw_properties_new(
        PW_KEY_NODE_NAME, name.c_str(), PW_KEY_NODE_GROUP, "mixyard-private",
        PW_KEY_NODE_ALWAYS_PROCESS, "true", PW_KEY_NODE_AUTOCONNECT, "false",
        "node.dont-reconnect", "true", "node.loop.name", "data-loop.0",
        PW_KEY_NODE_FORCE_RATE, "48000", PW_KEY_NODE_FORCE_QUANTUM, "128",
        nullptr);
    if (n >= 2) {
      pw_properties_set(props, PW_KEY_MEDIA_CLASS, "Audio/Source");
      pw_properties_set(props, PW_KEY_NODE_DESCRIPTION,
                        g->outputs[n - 2].label.c_str());
      pw_properties_set(props, "mixyard.output.id",
                        g->outputs[n - 2].id.c_str());
      pw_properties_set(props, "mixyard.mix.id", g->outputs[n - 2].mix.c_str());
    }
    e->filter = pw_filter_new(a->core, name.c_str(), props);
    require(e->filter, "owner-failed", "native filter creation failed");
    pw_filter_add_listener(e->filter, &e->hook, &filter_events, e);
    size_t ni = n == 0   ? 0
                : n == 1 ? g->ns
                         : 1,
           no = n == 0   ? g->ns
                : n == 1 ? g->no
                         : 1;
    for (size_t i = 0; i < ni; i++) {
      if (!continue_build(a))
        return;
      e->in.push_back(add_port(e, false, (unsigned)i));
    }
    for (size_t i = 0; i < no; i++) {
      if (!continue_build(a))
        return;
      e->out.push_back(add_port(e, true, (unsigned)i));
    }
  }
  g->sealed.store(true, std::memory_order_release);
  for (size_t n = 0; n < 2 + g->no; n++) {
    if (!continue_build(a))
      return;
    require(pw_filter_connect(g->nodes[n].filter, PW_FILTER_FLAG_RT_PROCESS,
                              nullptr, 0) >= 0,
            "owner-failed", "filter connect failed");
  }
}

static Global *global(App *a, uint32_t id) {
  for (auto &g : a->globals)
    if (g->alive && g->id == id)
      return g.get();
  return nullptr;
}
static EndpointId observed(App *a, EndpointNode *e, bool out, unsigned i) {
  uint32_t node = pw_filter_get_node_id(e->filter);
  auto n = global(a, node);
  require(n && n->is_node, "endpoint-changed", "native node not observed yet");
  std::string name = (out ? "out-" : "in-") + std::to_string(i);
  Global *found = nullptr;
  for (auto &p : a->globals)
    if (p->alive && p->is_port && p->node == node && p->input != out &&
        p->name == name) {
      require(!found, "endpoint-changed", "ambiguous own port");
      found = p.get();
    }
  require(found, "endpoint-changed", "native port not observed yet");
  return {node, found->id, n->serial, found->serial};
}
static bool endpoint_exists(App *a, const EndpointId &e, bool input,
                            bool negotiated) {
  auto n = global(a, e.node), p = global(a, e.port);
  return n && p && n->is_node && p->is_port && p->node == e.node &&
         p->input == input && n->serial == e.node_serial &&
         p->serial == e.port_serial && (!negotiated || p->format_ok);
}
static void link_info(void *data, const pw_link_info *info) {
  auto l = (LinkRecord *)data;
  if (info->change_mask & PW_LINK_CHANGE_MASK_FORMAT)
    l->format_ok = exact_format(info->format);
  bool ok = info->output_node_id == l->from.node &&
            info->output_port_id == l->from.port &&
            info->input_node_id == l->to.node &&
            info->input_port_id == l->to.port &&
            info->state == PW_LINK_STATE_ACTIVE && l->format_ok;
  l->active.store(ok, std::memory_order_release);
}
static const pw_link_events link_events = {.version = PW_VERSION_LINK_EVENTS,
                                           .info = link_info};
static void make_link(App *a, LinkRecord *l, const EndpointId &from,
                      const EndpointId &to) {
  require(endpoint_exists(a, from, false, false) &&
              endpoint_exists(a, to, true, false),
          "endpoint-changed", "link endpoint identity changed");
  l->from = from;
  l->to = to;
  auto props = pw_properties_new(nullptr, nullptr);
  pw_properties_setf(props, PW_KEY_LINK_OUTPUT_NODE, "%u", from.node);
  pw_properties_setf(props, PW_KEY_LINK_OUTPUT_PORT, "%u", from.port);
  pw_properties_setf(props, PW_KEY_LINK_INPUT_NODE, "%u", to.node);
  pw_properties_setf(props, PW_KEY_LINK_INPUT_PORT, "%u", to.port);
  pw_properties_set(props, PW_KEY_LINK_PASSIVE, "false");
  l->proxy = (pw_proxy *)pw_core_create_object(
      a->core, "link-factory", PW_TYPE_INTERFACE_Link, PW_VERSION_LINK,
      &props->dict, 0);
  pw_properties_free(props);
  require(l->proxy, "owner-failed", "native link creation failed");
  pw_link_add_listener((pw_link *)l->proxy, &l->hook, &link_events, l);
}
static bool link_internal(Generation *g) {
  auto a = g->app;
  try {
    for (size_t s = 0; s < g->ns; s++) {
      auto out = observed(a, &g->nodes[0], true, (unsigned)s),
           in = observed(a, &g->nodes[1], false, (unsigned)s);
      auto l = &g->links[s];
      if (!l->proxy)
        make_link(a, l, out, in);
    }
    for (size_t o = 0; o < g->no; o++) {
      auto out = observed(a, &g->nodes[1], true, (unsigned)o),
           in = observed(a, &g->nodes[2 + o], false, 0);
      auto l = &g->links[g->ns + o];
      if (!l->proxy)
        make_link(a, l, out, in);
      g->outputs[o].published = observed(a, &g->nodes[2 + o], true, 0);
    }
  } catch (const Error &e) {
    if (e.code == "endpoint-changed")
      return false;
    throw;
  }
  return true;
}
static void external_format(void *data, int seq, uint32_t id, uint32_t index,
                            uint32_t next, const spa_pod *param) {
  (void)seq;
  (void)index;
  (void)next;
  auto p = (Global *)data;
  if (id != SPA_PARAM_Format)
    return;
  bool ok = exact_format(param);
  bool changed = p->format_ok != ok;
  p->format_ok = ok;
  if (changed)
    for (auto &entry : p->app->generations) {
      auto g = entry.get();
      for (size_t o = 0; o < g->no; o++)
        if (g->outputs[o].bound && g->outputs[o].consumer.port == p->id) {
          g->inventory = ++p->app->inventory;
          if (!ok && g->outputs[o].link_replied)
            g->outputs[o].closed.store(true, std::memory_order_release);
        }
    }
}
static void external_port_info(void *data, const pw_port_info *info) {
  auto p = (Global *)data;
  for (uint32_t i = 0; i < info->n_params; i++)
    if (info->params[i].id == SPA_PARAM_Format &&
        (info->params[i].flags & SPA_PARAM_INFO_READ))
      if (pw_port_enum_params((pw_port *)p->proxy, 0, SPA_PARAM_Format, 0, 1,
                              nullptr) < 0)
        p->app->fault.store(21, std::memory_order_release);
}
static const pw_port_events port_events = {.version = PW_VERSION_PORT_EVENTS,
                                           .info = external_port_info,
                                           .param = external_format};
static const char *modules[] = {
    "libpipewire-module-protocol-native", "libpipewire-module-client-node",
    "libpipewire-module-access",          "libpipewire-module-spa-node-factory",
    "libpipewire-module-link-factory",    "libpipewire-module-metadata"};
static void added(void *data, uint32_t id, uint32_t permissions,
                  const char *type, uint32_t version, const spa_dict *props) {
  (void)permissions;
  (void)version;
  auto a = (App *)data;
  try {
    if (!strcmp(type, PW_TYPE_INTERFACE_Device))
      throw Error("unsupported-private-target", "device in private graph");
    bool node = !strcmp(type, PW_TYPE_INTERFACE_Node),
         port = !strcmp(type, PW_TYPE_INTERFACE_Port),
         module = !strcmp(type, PW_TYPE_INTERFACE_Module);
    if (!node && !port && !module)
      return;
    require(props, "unsupported-private-target", "missing registry properties");
    if (!strcmp(type, PW_TYPE_INTERFACE_Module)) {
      const char *m = spa_dict_lookup(props, PW_KEY_MODULE_NAME);
      unsigned bit = 0;
      for (unsigned i = 0; i < 6; i++)
        if (m && !strcmp(m, modules[i])) {
          bit = 1u << i;
          a->module_ids[i] = id;
        }
      require(bit && !(a->modules & bit), "unqualified-sdk",
              "unexpected or duplicate private module");
      a->modules |= bit;
      return;
    }
    require(a->globals.size() < a->global_budget, "resource-limit",
            "registry budget exceeded");
    auto p = std::make_unique<Global>();
    p->app = a;
    p->id = id;
    p->serial = decimal(spa_dict_lookup(props, PW_KEY_OBJECT_SERIAL));
    require(p->serial, "endpoint-changed", "zero serial");
    p->is_node = node;
    p->is_port = port;
    p->alive = true;
    if (node) {
      const char *n = spa_dict_lookup(props, PW_KEY_NODE_NAME);
      require(n && !strncmp(n, "mixyard-private-", 16),
              "unsupported-private-target", "foreign node in private core");
      p->name = n;
    } else {
      uint64_t n = decimal(spa_dict_lookup(props, PW_KEY_NODE_ID));
      require(n < SPA_ID_INVALID, "endpoint-changed", "node ID overflow");
      p->node = (uint32_t)n;
      const char *direction = spa_dict_lookup(props, PW_KEY_PORT_DIRECTION);
      require(direction &&
                  (!strcmp(direction, "in") || !strcmp(direction, "out")),
              "endpoint-changed", "unknown port direction");
      p->input = !strcmp(direction, "in");
      const char *name = spa_dict_lookup(props, PW_KEY_PORT_NAME);
      require(name && strlen(name) <= 4096, "endpoint-changed",
              "port name bound");
      p->name = name;
      p->proxy = (pw_proxy *)pw_registry_bind(
          a->registry, id, PW_TYPE_INTERFACE_Port, PW_VERSION_PORT, 0);
      require(p->proxy, "owner-failed", "port bind failed");
      pw_port_add_listener((pw_port *)p->proxy, &p->hook, &port_events,
                           p.get());
      uint32_t param = SPA_PARAM_Format;
      require(pw_port_subscribe_params((pw_port *)p->proxy, &param, 1) >= 0,
              "owner-failed", "format subscription failed");
    }
    a->globals.push_back(std::move(p));
  } catch (const Error &e) {
    a->last_code = e.code;
    a->last_message = e.what();
    a->fault.store(22, std::memory_order_release);
    a->admission.store(false, std::memory_order_release);
  }
}
static void removed(void *data, uint32_t id) {
  auto a = (App *)data;
  for (auto module : a->module_ids)
    if (module == id) {
      a->fault.store(27, std::memory_order_release);
      a->admission.store(false, std::memory_order_release);
    }
  auto p = global(a, id);
  if (p) {
    p->alive = false;
    p->format_ok = false;
  }
  for (auto &entry : a->generations) {
    auto g = entry.get();
    if (g->objects_destroyed)
      continue;
    for (size_t o = 0; o < g->no; o++)
      if (g->outputs[o].bound && (g->outputs[o].consumer.node == id ||
                                  g->outputs[o].consumer.port == id))
        g->outputs[o].closed.store(true, std::memory_order_release);
    if (g->nodes)
      for (size_t n = 0; n < 2; n++)
        if (g->nodes[n].filter) {
          auto own = pw_filter_get_node_id(g->nodes[n].filter);
          if (own == id || (p && p->is_port && p->node == own))
            rt_fault(g, 23);
        }
  }
}
static const pw_registry_events registry_events = {
    .version = PW_VERSION_REGISTRY_EVENTS,
    .global = added,
    .global_remove = removed};
static void core_info(void *data, const pw_core_info *info) {
  auto a = (App *)data;
  const char *name = info->name;
  a->core_ok = info->version && !strcmp(info->version, "1.4.8") && name &&
               !strcmp(name, "mixyard-private");
  if (!a->core_ok) {
    a->fault.store(24, std::memory_order_release);
    a->admission.store(false, std::memory_order_release);
  }
}
static void core_error(void *data, uint32_t id, int seq, int result,
                       const char *message) {
  (void)id;
  (void)seq;
  (void)result;
  (void)message;
  auto a = (App *)data;
  a->fault.store(25, std::memory_order_release);
  a->admission.store(false, std::memory_order_release);
}
static const pw_core_events core_events = {
    .version = PW_VERSION_CORE_EVENTS, .info = core_info, .error = core_error};

static json_object *event(const char *kind) {
  auto j = object();
  add(j, "kind", kind);
  return j;
}
static void emit(App *a, const std::string &op, json_object *e) {
  Json j(object());
  add(j.get(), "companionVersion", uint64_t(1));
  add(j.get(), "instanceNonce", a->nonce);
  add(j.get(), "controlEpoch", a->epoch.load(std::memory_order_acquire));
  add(j.get(), "operationId", op);
  add(j.get(), "event", e);
  std::string s = dump(j.get()) + "\n";
  require(s.size() <= wire::limit &&
              a->queued_bytes + s.size() <= 4 * wire::limit,
          "resource-limit", "bounded reply queue exhausted");
  a->queued_bytes += s.size();
  a->replies.push_back(std::move(s));
}
static void fault_reply(App *a, const std::string &op, const std::string &code,
                        const std::string &why) {
  a->last_code = code;
  a->last_message = why;
  auto e = event("fault"), error = object();
  add(error, "code", code);
  add(error, "message", why);
  add(e, "error", error);
  emit(a, op, e);
}
static json_object *outputs_json(Generation *g) {
  auto list = json_object_new_array();
  if (!g)
    return list;
  for (size_t o = 0; o < g->no; o++) {
    auto &out = g->outputs[o];
    auto j = object();
    add(j, "outputId", out.id);
    add(j, "mixId", out.mix);
    add(j, "label", out.label);
    add(j, "endpoint", endpoint_json(out.published));
    add(j, "format", format_json());
    add(j, "linkedConsumer", out.bound ? endpoint_json(out.consumer) : nullptr);
    json_object_array_add(list, j);
  }
  return list;
}
static json_object *installed(Generation *g) {
  require(g && g->complete.load(std::memory_order_acquire), "owner-failed",
          "full native receipt unavailable");
  auto a = g->app;
  auto j = object();
  add(j, "instanceNonce", a->nonce);
  add(j, "controlEpoch", g->epoch);
  add(j, "operationId", g->install_op);
  add(j, "generation", g->generation);
  add(j, "storeRevision", g->store);
  add(j, "configRevision", g->revision);
  add(j, "inventoryRevision", g->admitted_inventory);
  add(j, "requestDigest", g->request_digest);
  add(j, "endpointSetDigest", g->endpoint_digest);
  add(j, "clockId", g->clock);
  add(j, "nativeSwitchFrame", g->switch_frame.load(std::memory_order_acquire));
  add(j, "engineFirstFrame", uint64_t(0));
  add(j, "firstProcessedFrame",
      g->first_processed.load(std::memory_order_acquire));
  auto commits = json_object_new_array();
  for (size_t o = 0; o < g->no; o++) {
    auto c = object();
    add(c, "outputId", g->outputs[o].id);
    add(c, "nativeFrame",
        g->outputs[o].first_commit.load(std::memory_order_acquire));
    json_object_array_add(commits, c);
  }
  add(j, "outputCommits", commits);
  boolean(j, "ownerInstallTerminal", true);
  boolean(j, "noLateInstall", true);
  return j;
}
static json_object *switch_evidence(Generation *g) {
  if (!g || !g->switched.load(std::memory_order_acquire))
    return nullptr;
  auto j = object();
  add(j, "instanceNonce", g->app->nonce);
  add(j, "controlEpoch", g->epoch);
  add(j, "operationId", g->install_op);
  add(j, "generation", g->generation);
  add(j, "configRevision", g->revision);
  add(j, "inventoryRevision", g->admitted_inventory);
  add(j, "clockId", g->clock);
  add(j, "nativeSwitchFrame", g->switch_frame.load(std::memory_order_acquire));
  return j;
}
static mx_private_stop_tuple stop_tuple(App *a) {
  mx_private_stop_tuple t{};
  t.epoch = a->epoch.load(std::memory_order_acquire);
  t.active = a->published_generation;
  if (a->fencing && a->fence_owner_done.load(std::memory_order_acquire)) {
    t.active = 0;
    return t;
  }
  if (a->control.present && !a->control.completed) {
    t.pending = true;
    t.reserved = a->control.target.reserved;
    memcpy(t.operation, a->control.target.operation, sizeof(t.operation));
  }
  return t;
}
static mx_private_stop_tuple parse_stop(App *a, json_object *j) {
  keys(j,
       {"instanceNonce", "controlEpoch", "activeGeneration", "pendingInstall"});
  require(str(get(j, "instanceNonce")) == a->nonce, "stale-instance",
          "stop instance mismatch");
  mx_private_stop_tuple t{};
  t.epoch = num(get(j, "controlEpoch"));
  auto active = get(j, "activeGeneration"), pending = get(j, "pendingInstall");
  if (active)
    t.active = num(active);
  if (pending) {
    keys(pending, {"operationId", "reservedGeneration"});
    auto op = str(get(pending, "operationId"));
    require(op.size() <= 64, "protocol-error", "operation ID bound");
    t.pending = true;
    t.reserved = num(get(pending, "reservedGeneration"));
    memcpy(t.operation, op.c_str(), op.size() + 1);
  }
  return t;
}
static json_object *stop_json(App *a) {
  auto t = stop_tuple(a);
  auto j = object();
  add(j, "instanceNonce", a->nonce);
  add(j, "controlEpoch", t.epoch);
  add(j, "activeGeneration",
      t.active ? json_object_new_uint64(t.active) : nullptr);
  auto p = t.pending ? object() : nullptr;
  if (p) {
    add(p, "operationId", t.operation);
    add(p, "reservedGeneration", t.reserved);
  }
  add(j, "pendingInstall", p);
  return j;
}
static json_object *status(App *a) {
  Generation *g = a->candidate;
  auto j = object();
  add(j, "scope", "owned-private-generated-v1");
  // Only the product supervisor publishes registered child connection metadata.
  add(j, "privateCore", static_cast<json_object *>(nullptr));
  add(j, "phase", a->phase);
  add(j, "stopTarget", stop_json(a));
  add(j, "desiredStoreRevision",
      g ? json_object_new_uint64(g->store) : nullptr);
  add(j, "desiredConfigRevision",
      g ? json_object_new_uint64(g->revision) : nullptr);
  add(j, "inventoryRevision",
      g ? json_object_new_uint64(g->inventory.load()) : nullptr);
  add(j, "preparedToken",
      g && g->prepare_replied && !a->fencing
          ? json_object_new_string(g->prepared.c_str())
          : nullptr);
  add(j, "installed",
      g && !g->retired.load(std::memory_order_acquire) &&
              g->complete.load(std::memory_order_acquire)
          ? installed(g)
          : nullptr);
  add(j, "outputs", outputs_json(g && !g->objects_destroyed ? g : nullptr));
  auto pending = !a->fence_owner_done.load(std::memory_order_acquire) &&
                         a->ticket && !a->ticket->install_replied
                     ? a->ticket
                     : nullptr;
  add(j, "pendingOperation",
      pending ? json_object_new_string(pending->install_op.c_str()) : nullptr);
  auto error = a->last_code.empty() ? nullptr : object();
  if (error) {
    add(error, "code", a->last_code);
    add(error, "message", a->last_message);
  }
  add(j, "lastError", error);
  return j;
}
static std::string observed_id(const EndpointId &e) {
  return "owned-private-generated-v1:node:" + std::to_string(e.node) + ":" +
         std::to_string(e.node_serial) + ":port:" + std::to_string(e.port) +
         ":" + std::to_string(e.port_serial);
}
static json_object *spec(const EndpointId &e, bool source) {
  auto j = object();
  add(j, "observedId", observed_id(e));
  if (source)
    add(j, "encoding", "f64-le");
  add(j, "sampleRateHz", uint64_t(48000));
  auto channels = json_object_new_array();
  json_object_array_add(channels, json_object_new_string("FL"));
  json_object_array_add(channels, json_object_new_string("FR"));
  add(j, "channels", channels);
  add(j, "mapping", json_object_get(channels));
  return j;
}
static void canonical_request(Generation *g) {
  auto a = g->app;
  Json request(object());
  add(request.get(), "config", json_object_get(g->config.get()));
  auto inventory = object();
  add(inventory, "schemaVersion", uint64_t(2));
  add(inventory, "revision", g->admitted_inventory);
  add(inventory, "platform", "linux");
  add(inventory, "evidence", "simulated");
  auto inventory_sources = json_object_new_array(),
       inventory_outputs = json_object_new_array(),
       sources = json_object_new_array(), outputs = json_object_new_array(),
       endpoints = json_object_new_array();
  for (size_t s = 0; s < g->ns; s++) {
    auto id = observed(a, &g->nodes[0], true, (unsigned)s);
    auto item = object(), fmt = object();
    add(item, "observedId", observed_id(id));
    // The canonical selector is a policy alias within this generated-only
    // scope. No PID/application discovery or OS identity claim is made.
    add(item, "selector",
        json_object_get(get(g->sources[s].config, "selector")));
    add(item, "origin", "external");
    add(fmt, "channels", uint64_t(2));
    add(fmt, "sampleRateHz", uint64_t(48000));
    add(item, "format", fmt);
    json_object_array_add(inventory_sources, item);
    json_object_array_add(sources, spec(id, true));
    json_object_array_add(endpoints, endpoint_json(id));
  }
  for (size_t o = 0; o < g->no; o++) {
    auto &out = g->outputs[o];
    require(endpoint_exists(a, out.consumer, true, true), "endpoint-changed",
            "consumer serial or negotiated format changed");
    auto item = object(), fmt = object();
    add(item, "observedId", observed_id(out.consumer));
    add(item, "target", json_object_get(get(out.config, "target")));
    add(fmt, "channels", uint64_t(2));
    add(fmt, "sampleRateHz", uint64_t(48000));
    add(item, "format", fmt);
    json_object_array_add(inventory_outputs, item);
    json_object_array_add(outputs, spec(out.consumer, false));
    auto binding = object();
    add(binding, "outputId", out.id);
    add(binding, "published", endpoint_json(out.published));
    add(binding, "consumer", endpoint_json(out.consumer));
    add(binding, "format", format_json());
    json_object_array_add(endpoints, binding);
  }
  Json endpoint_list(endpoints);
  g->endpoint_digest = digest(dump(endpoint_list.get()));
  add(inventory, "sources", inventory_sources);
  add(inventory, "outputs", inventory_outputs);
  add(request.get(), "inventory", inventory);
  add(request.get(), "generation", g->generation);
  add(request.get(), "clockId", g->clock);
  add(request.get(), "maxFrames", uint64_t(frames));
  auto limits = object();
  add(limits, "maxSources", g->ns);
  add(limits, "maxMixes", g->nm);
  add(limits, "maxSends", count(get(g->config.get(), "sends")));
  add(limits, "maxOutputs", g->no);
  add(limits, "maxBufferSamples",
      uint64_t((g->ns + g->nm + g->no) * frames * 2));
  add(limits, "maxTotalBytes", g->budget / 2);
  add(limits, "maxDspWorkUnitsPerBlock", uint64_t(1000000));
  add(request.get(), "limits", limits);
  add(request.get(), "sources", sources);
  add(request.get(), "outputs", outputs);
  g->request = dump(request.get());
  require(g->request.size() <= wire::limit, "resource-limit",
          "canonical host request limit");
  g->request_digest = digest(g->request);
}
static bool compiled_spec(mx_host *h, bool output, size_t index,
                          json_object *expected) {
  uint8_t text[8192];
  size_t needed = 0;
  int result = output
                   ? mx_host_output_info(h, index, text, sizeof(text), &needed)
                   : mx_host_source_info(h, index, text, sizeof(text), &needed);
  if (result != MX_OK || needed >= sizeof(text))
    return false;
  try {
    auto j = wire::parse(std::string((char *)text, needed));
    return json_object_equal(j.get(), expected);
  } catch (...) {
    return false;
  }
}
static void *builder(void *data) {
  auto b = (Build *)data;
  auto g = b->g;
  try {
    b->result =
        mx_host_create((const uint8_t *)g->request.data(), g->request.size(),
                       &b->host, b->error, sizeof(b->error));
    if (b->result == MX_OK) {
      auto request = wire::parse(g->request);
      bool valid = mx_host_source_count(b->host) == g->ns &&
                   mx_host_output_count(b->host) == g->no;
      for (size_t i = 0; i < g->ns && valid; i++)
        valid = compiled_spec(b->host, false, i,
                              at(get(request.get(), "sources"), i));
      for (size_t i = 0; i < g->no && valid; i++)
        valid = compiled_spec(b->host, true, i,
                              at(get(request.get(), "outputs"), i));
      if (!valid) {
        mx_host_destroy(b->host);
        b->host = nullptr;
        b->result = MX_INVALID;
        strcpy(b->error, "compiled source/output bindings differ");
      }
    }
  } catch (...) {
    if (b->host)
      mx_host_destroy(b->host);
    b->host = nullptr;
    b->result = MX_INVALID;
    strcpy(b->error, "host construction failed");
  }
  b->done.store(true, std::memory_order_release);
  return nullptr;
}
static bool all_bindings(Generation *g);
static void collect_builder(App *a) {
  auto &b = a->build;
  if (!b.started || !b.done.load(std::memory_order_acquire))
    return;
  if (pthread_tryjoin_np(b.thread, nullptr) != 0)
    return; // never wait behind compilation
  b.started = false;
  auto g = b.g;
  if (a->fencing || a->closing || !current(g)) {
    if (b.host)
      mx_host_destroy(b.host);
    b.host = nullptr;
    return;
  }
  if (b.result != MX_OK || !b.host) {
    fault_reply(a, g->stage_op, "owner-failed",
                b.error[0] ? b.error : "canonical host construction rejected");
    return;
  }
  if (g->inventory.load(std::memory_order_acquire) != g->admitted_inventory ||
      !all_bindings(g) || !g->headers.load(std::memory_order_acquire) ||
      g->seen_clock.load(std::memory_order_acquire) != g->clock) {
    mx_host_destroy(b.host);
    b.host = nullptr;
    fault_reply(a, g->stage_op, "endpoint-changed",
                "native binding/clock changed during host construction");
    return;
  }
  g->staged_host = b.host;
  b.host = nullptr;
  g->staged =
      digest(a->nonce + ":" + std::to_string(g->epoch) + ":" + g->stage_op +
             ":" + g->request_digest + ":" + g->endpoint_digest);
  auto e = event("staged");
  add(e, "stagedToken", g->staged);
  add(e, "generation", g->generation);
  add(e, "configRevision", g->revision);
  add(e, "inventoryRevision", g->admitted_inventory);
  add(e, "requestDigest", g->request_digest);
  add(e, "endpointSetDigest", g->endpoint_digest);
  emit(a, g->stage_op, e);
  a->phase = "prepared";
}

static void prepare(App *a, const std::string &op, json_object *action,
                    const std::string &raw_config) {
  keys(action, {"kind", "generation", "snapshot"});
  require(!a->candidate && !a->fencing && !a->closing && !a->pending.load(),
          "busy", "generation already reserved");
  auto snapshot = get(action, "snapshot");
  keys(snapshot, {"storeRevision", "configRevision", "configSha256", "config"});
  auto sha = str(get(snapshot, "configSha256"));
  require(hex(sha) && !raw_config.empty() && digest(raw_config) == sha,
          "stale-config", "exact transmitted Config hash differs");
  auto g = std::make_unique<Generation>();
  g->app = a;
  g->generation = num(get(action, "generation"));
  g->epoch = a->epoch.load();
  g->store = num(get(snapshot, "storeRevision"));
  g->revision = num(get(snapshot, "configRevision"));
  g->inventory = ++a->inventory;
  g->prepare_op = op;
  require(g->revision &&
              num(get(get(snapshot, "config"), "revision")) == g->revision,
          "stale-config", "Config revision mismatch");
  g->config = Json(json_object_get(get(snapshot, "config")));
  auto sources = get(g->config.get(), "sources"),
       outputs = get(g->config.get(), "outputs"),
       mixes = get(g->config.get(), "mixes");
  g->ns = count(sources);
  g->no = count(outputs);
  g->nm = count(mixes);
  require(g->ns && g->ns <= a->max_sources && g->no &&
              g->no <= a->max_outputs && g->nm,
          "resource-limit", "dynamic graph exceeds descriptor budgets");
  // Conservative reservation covers JSON objects, native registrations,
  // requested SDK pools, owned scratch and the host's explicit PCM budget.
  uint64_t estimate = 2 * wire::limit + raw_config.size() * 32 +
                      (g->ns + g->no + g->nm) * 65536;
  require(estimate <= a->max_bytes - a->resident, "resource-limit",
          "resident reservation exhausted");
  g->budget = estimate;
  a->resident += estimate;
  require(mx_private_reserve(&a->last_generation, g->generation),
          "stale-generation", "generation reservation reused");
  g->sources = std::make_unique<Source[]>(g->ns);
  g->outputs = std::make_unique<Output[]>(g->no);
  g->inputs = std::make_unique<mx_input[]>(g->ns);
  std::map<std::string, json_object *> source_by_id, output_by_id, mix_by_id,
      generator_by_id;
  for (size_t i = 0; i < g->ns; i++) {
    auto s = at(sources, i);
    auto id = str(get(s, "id"));
    require(source_by_id.emplace(id, s).second, "protocol-error",
            "duplicate Config source ID");
  }
  for (size_t i = 0; i < g->no; i++) {
    auto o = at(outputs, i);
    auto id = str(get(o, "id"));
    require(output_by_id.emplace(id, o).second, "protocol-error",
            "duplicate Config output ID");
    auto target = get(o, "target");
    require(str(get(target, "kind")) == "virtual-input",
            "unsupported-private-target",
            "private publication requires virtual-input target");
  }
  for (size_t i = 0; i < g->nm; i++) {
    auto m = at(mixes, i);
    require(num(get(m, "channels")) == 2 &&
                num(get(m, "sampleRateHz")) == 48000,
            "unsupported-private-format", "mix format must be 48000 stereo");
    require(mix_by_id.emplace(str(get(m, "id")), m).second, "protocol-error",
            "duplicate mix ID");
  }
  auto generators = get(a->descriptor.get(), "generators");
  for (size_t i = 0; i < count(generators); i++) {
    auto gen = at(generators, i);
    generator_by_id.emplace(str(get(gen, "sourceId")), gen);
  }
  require(source_by_id.size() == generator_by_id.size(),
          "unsupported-private-target",
          "every Config source must have exactly one generated binding");
  size_t i = 0;
  for (auto &entry : source_by_id) {
    auto found = generator_by_id.find(entry.first);
    require(found != generator_by_id.end(), "unsupported-private-target",
            "unbound generated source ID");
    auto &s = g->sources[i++];
    s.id = entry.first;
    s.config = entry.second;
    s.left = json_object_get_double(get(found->second, "left"));
    s.right = json_object_get_double(get(found->second, "right"));
    s.marker = num(get(found->second, "markerPeriod"));
    s.limit = num(get(found->second, "frameLimit"));
  }
  i = 0;
  for (auto &entry : output_by_id) {
    auto &out = g->outputs[i++];
    out.id = entry.first;
    out.config = entry.second;
    out.mix = str(get(entry.second, "mix"));
    auto mix = mix_by_id.find(out.mix);
    require(mix != mix_by_id.end(), "protocol-error",
            "output references unknown mix");
    out.label = str(get(mix->second, "name"));
  }
  g->prepared = digest(a->nonce + ":" + std::to_string(g->epoch) + ":" + op +
                       ":" + sha + ":" + std::to_string(g->generation));
  g->warming.store(true, std::memory_order_release);
  auto ptr = g.get();
  a->generations.push_back(std::move(g));
  a->candidate = ptr;
  a->phase = "preparing";
  create_graph(ptr);
}
static bool all_bindings(Generation *g) {
  auto a = g->app;
  if (!graph_formats(g))
    return false;
  for (size_t i = 0; i < g->ns + 2 * g->no; i++)
    if (!g->links[i].active.load(std::memory_order_acquire))
      return false;
  for (size_t o = 0; o < g->no; o++)
    if (!g->outputs[o].bound ||
        g->outputs[o].closed.load(std::memory_order_acquire) ||
        !endpoint_exists(a, g->outputs[o].consumer, true, true))
      return false;
  return true;
}
static int fault_owner(spa_loop *loop, bool async, uint32_t seq,
                       const void *data, size_t size, void *userdata) {
  (void)loop;
  (void)async;
  (void)seq;
  (void)data;
  (void)size;
  auto a = (App *)userdata;
  if (!pw_data_loop_in_thread(a->owner))
    return -EPERM;
  for (auto &entry : a->generations)
    revoke(entry.get());
  a->active = nullptr;
  return 0;
}
static int fence_owner(spa_loop *loop, bool async, uint32_t seq,
                       const void *data, size_t size, void *userdata) {
  (void)loop;
  (void)async;
  (void)seq;
  (void)data;
  (void)size;
  auto a = (App *)userdata;
  if (!pw_data_loop_in_thread(a->owner))
    return -EPERM;
  // Ticket userdata and every host survive this barrier. Builders cannot
  // publish tickets; their completion is collected only by main control.
  for (auto &entry : a->generations)
    revoke(entry.get());
  a->active = nullptr;
  a->active_generation.store(0, std::memory_order_release);
  a->pending.store(nullptr, std::memory_order_release);
  a->fence_owner_done.store(true, std::memory_order_release);
  return 0;
}
static int cleanup_owner(spa_loop *loop, bool async, uint32_t seq,
                         const void *data, size_t size, void *userdata) {
  (void)loop;
  (void)async;
  (void)seq;
  (void)data;
  (void)size;
  auto a = (App *)userdata;
  if (!pw_data_loop_in_thread(a->owner))
    return -EPERM;
  a->cleanup_done.store(true, std::memory_order_release);
  return 0;
}
static void begin_fence(App *a, const std::string &op, uint64_t epoch,
                        Generation *canceled = nullptr) {
  require(!a->fencing && epoch > a->epoch.load() && epoch != UINT64_MAX,
          "stale-control-epoch", "fence epoch must advance once");
  a->admission.store(false, std::memory_order_release);
  a->epoch.store(epoch, std::memory_order_release);
  a->fence_epoch = epoch;
  a->fence_op = op;
  a->fence_pending = a->fence_pending || (a->control.present && !a->control.completed);
  a->control.fenced = true;
  a->fence_ticket = canceled ? canceled : (a->fence_pending ? a->candidate : nullptr);
  a->fencing = true;
  a->phase = "stopping";
  int r = pw_data_loop_invoke(a->owner, fence_owner, 0, nullptr, 0, false, a);
  if (r < 0) {
    a->failed_held = true;
    a->phase = "failed-held";
    fault_reply(a, op, "stop-incomplete",
                "native owner retirement could not be queued");
  } else
    a->fence_owner_queued = true;
}
static bool scrubbed(App *a) {
  if (!a->fence_owner_done.load(std::memory_order_acquire) || a->build.started)
    return false;
  for (auto &entry : a->generations) {
    auto g = entry.get();
    if (g->scrub_failed.load(std::memory_order_acquire))
      return false;
    if (!g->nodes)
      continue;
    for (size_t n = 0; n < 2 + g->no; n++) {
      auto &e = g->nodes[n];
      for (auto p : e.in)
        for (auto &r : p->buffers)
          if (r.token.load(std::memory_order_acquire) &&
              r.zero_epoch.load(std::memory_order_acquire) < a->fence_epoch)
            return false;
      for (auto p : e.out)
        for (auto &r : p->buffers)
          if (r.token.load(std::memory_order_acquire) &&
              r.zero_epoch.load(std::memory_order_acquire) < a->fence_epoch)
            return false;
    }
  }
  return true;
}
static void destroy_objects(Generation *g) {
  if (g->objects_destroyed)
    return;
  g->objects_destroyed = true;
  if (g->links)
    for (size_t i = 0; i < g->ns + 2 * g->no; i++)
      if (g->links[i].proxy) {
        pw_proxy_destroy(g->links[i].proxy);
        g->links[i].proxy = nullptr;
      }
  if (g->nodes)
    for (size_t n = 0; n < 2 + g->no; n++)
      if (g->nodes[n].filter) {
        pw_filter_destroy(g->nodes[n].filter);
        g->nodes[n].filter = nullptr;
      }
}
static void finish_fence(App *a) {
  if (!a->fencing || a->fence_replied || a->failed_held)
    return;
  if (a->fault.load(std::memory_order_acquire)) {
    a->failed_held = true;
    a->phase = "failed-held";
    fault_reply(a, a->fence_op, "stop-incomplete",
                "private SDK ownership failed; no scrub proof");
    return;
  }
  if (!a->cleanup_queued && scrubbed(a)) {
    for (auto &g : a->generations)
      destroy_objects(g.get());
    a->cleanup_queued = true;
    if (pw_data_loop_invoke(a->owner, cleanup_owner, 0, nullptr, 0, false, a) <
        0) {
      a->failed_held = true;
      a->phase = "failed-held";
      fault_reply(a, a->fence_op, "stop-incomplete",
                  "SDK withdrawal barrier failed");
    }
    return;
  }
  if (!a->cleanup_done.load(std::memory_order_acquire))
    return;
  for (auto &entry : a->generations)
    if (entry->scrub_failed.load(std::memory_order_acquire)) {
      a->failed_held = true;
      a->phase = "failed-held";
      fault_reply(a, a->fence_op, "stop-incomplete",
                  "SDK buffer withdrawn without proven zero borrow");
      return;
    }
  for (auto &entry : a->generations) {
    auto g = entry.get();
    if (g->staged_host) {
      mx_host_destroy(g->staged_host);
      g->staged_host = nullptr;
    }
  }
  auto g = a->fence_ticket;
  auto r = object();
  add(r, "instanceNonce", a->nonce);
  add(r, "controlEpoch", a->fence_epoch);
  add(r, "operationId", a->fence_op);
  add(r, "outcome",
      a->fence_pending ? (g && g->switched.load(std::memory_order_acquire)
               ? "aborted-after-switch"
               : "canceled-before-switch")
        : "no-pending");
  auto evidence_ticket = g ? g : a->settled_fence_ticket;
  add(r, "switchEvidence", switch_evidence(g));
  add(r, "switched",
      evidence_ticket &&
              evidence_ticket->complete.load(std::memory_order_acquire)
          ? installed(evidence_ticket)
          : nullptr);
  boolean(r, "ownerQuiesced", true);
  boolean(r, "pendingPcmZero", true);
  boolean(r, "noLateInstall", true);
  auto e = event("fenced");
  add(e, "receipt", r);
  emit(a, a->fence_op, e);
  a->fence_replied = true;
  a->published_generation = 0;
  a->phase = "stopped";
}

static mx_private_action reserved_action(const std::string &kind) {
  if (kind == "prepare") return MX_PRIVATE_PREPARE;
  if (kind == "link") return MX_PRIVATE_LINK;
  if (kind == "stage") return MX_PRIVATE_STAGE;
  return (mx_private_action)0;
}
static void handle(App *a, const std::string &line, bool priority) {
  std::string op = "invalid";
  try {
    std::string raw_config;
    auto request = wire::parse(line, &raw_config);
    keys(request.get(), {"companionVersion", "instanceNonce", "controlEpoch",
                         "operationId", "action"});
    require(num(get(request.get(), "companionVersion")) == 1, "protocol-error",
            "unsupported companion version");
    require(str(get(request.get(), "instanceNonce")) == a->nonce,
            "stale-instance", "instance nonce mismatch");
    op = str(get(request.get(), "operationId"));
    require(op.size() <= 64, "protocol-error", "operation ID bound");
    auto action = get(request.get(), "action");
    auto kind = str(get(action, "kind"));
    uint64_t epoch = num(get(request.get(), "controlEpoch"));
    // Only the exact received reservation's canceled normal bytes may drain
    // silently after Fence. All other old epochs remain protocol failures.
    if (epoch != a->epoch.load(std::memory_order_acquire) && a->fencing &&
        a->control.fenced && epoch == a->control.target.epoch &&
        op == a->control.target.operation) {
      if (mx_private_control_matches(&a->control, epoch, op.c_str(),
                                     reserved_action(kind), digest(line).c_str())) return;
      if (kind == "install" && a->control.action == MX_PRIVATE_STAGE &&
          a->control.consumed && a->ticket && !a->ticket->staged.empty()) {
        keys(action, {"kind", "stagedToken"});
        if (str(get(action, "stagedToken")) == a->ticket->staged &&
            op == a->ticket->stage_op) return;
      }
    }
    require(epoch == a->epoch.load(std::memory_order_acquire),
            "stale-control-epoch", "control epoch mismatch");
    require(!priority || kind == "reserve" || kind == "fence" || kind == "shutdown",
            "protocol-error", "priority socket only accepts reserve/fence/shutdown");
    if (kind == "reserve") {
      keys(action, {"kind", "actionKind", "reservedGeneration", "requestDigest"});
      require(priority && !a->fencing && !a->closing && !a->failed_held,
              "busy", "private control reservation closed");
      auto normal_kind = str(get(action, "actionKind"));
      auto normal_action = reserved_action(normal_kind);
      auto generation = num(get(action, "reservedGeneration"));
      auto hash = str(get(action, "requestDigest"));
      require(normal_action && hex(hash) && !op.empty() &&
                  a->operations.count(normal_kind + ":" + op) == 0,
              "protocol-error", "invalid or replayed reservation");
      if (normal_action == MX_PRIVATE_PREPARE) {
        require(!a->candidate && generation > a->last_generation && generation < UINT64_MAX,
                "stale-generation", "invalid Prepare reservation");
      } else {
        require(a->candidate && a->candidate->prepare_replied &&
                    generation == a->candidate->generation && !a->candidate->stage_started,
                "stale-generation", "reservation has no current prepared graph");
      }
      mx_private_stop_tuple target{};
      target.epoch = epoch; target.active = a->published_generation;
      target.pending = true; target.reserved = generation;
      memcpy(target.operation, op.c_str(), op.size() + 1);
      require(mx_private_control_reserve(&a->control, &target, normal_action, hash.c_str()),
              "busy", "another control reservation is retained");
      auto e = event("reserved");
      add(e, "reservedGeneration", generation); add(e, "requestDigest", hash);
      emit(a, op, e); return;
    }
    if (kind == "snapshot") {
      keys(action, {"kind"});
      auto e = event("snapshot");
      add(e, "status", status(a));
      emit(a, op, e);
      return;
    }
    if (kind == "fence") {
      keys(action, {"kind", "expected", "newControlEpoch"});
      auto expected = parse_stop(a, get(action, "expected"));
      auto actual = stop_tuple(a);
      require(mx_private_control_fence(&a->control, &expected, &actual),
              "stale-generation", "exact received reservation/active Stop target changed");
      a->fence_pending = expected.pending;
      a->settled_fence_ticket = !expected.pending && expected.active &&
                                        a->ticket &&
                                        a->ticket->generation == expected.active
                                    ? a->ticket
                                    : nullptr;
      begin_fence(a, op, num(get(action, "newControlEpoch")),
                  expected.pending ? a->ticket : nullptr);
      return;
    }
    if (kind == "shutdown") {
      keys(action, {"kind"});
      require(a->fencing && a->fence_replied && op == a->fence_op,
              "stop-incomplete",
              "Shutdown requires the current proven Fence operation");
      a->closing = true;
      a->shutdown_op = a->fence_op;
      return;
    }
    require(a->resident + 256 + kind.size() + op.size() <= a->max_bytes,
            "resource-limit", "control operation resident budget");
    require(a->operations.size() < 65536 &&
                a->operations.insert(kind + ":" + op).second,
            "protocol-error", "reused operation ID or operation budget");
    a->resident += 256 + kind.size() + op.size();
    require(a->ready && a->admission.load(std::memory_order_acquire) &&
                !a->fencing && !a->closing && !a->failed_held,
            "busy", "private owner not admitting mutations");
    if (kind != "install") {
      require(mx_private_control_consume(&a->control, epoch, op.c_str(),
                                         reserved_action(kind), digest(line).c_str()),
              "protocol-error", "normal bytes differ from received reservation");
    }
    if (kind == "prepare") {
      prepare(a, op, action, raw_config);
      return;
    }
    auto g = a->candidate;
    require(g && g->prepare_replied, "stale-generation",
            "no prepared generation");
    if (kind == "link") {
      keys(action, {"kind", "preparedToken", "outputId", "consumer"});
      require(!g->stage_started &&
                  str(get(action, "preparedToken")) == g->prepared,
              "stale-generation", "prepared token mismatch or sealed links");
      auto id = str(get(action, "outputId"));
      auto consumer = endpoint_parse(get(action, "consumer"));
      require(endpoint_exists(a, consumer, true, false), "endpoint-changed",
              "consumer not in observed private core");
      Output *out = nullptr;
      size_t i = 0;
      for (; i < g->no; i++)
        if (g->outputs[i].id == id) {
          out = &g->outputs[i];
          break;
        }
      require(out && !out->bound, "endpoint-changed",
              "unknown or already bound output");
      for (size_t o = 0; o < g->no; o++)
        require(!g->outputs[o].bound ||
                    !equal(g->outputs[o].consumer, consumer),
                "endpoint-changed", "consumer endpoint already bound");
      for (auto &entry : a->generations)
        if (entry->nodes)
          for (size_t n = 0; n < 2 + entry->no; n++)
            require(!entry->nodes[n].filter ||
                        pw_filter_get_node_id(entry->nodes[n].filter) !=
                            consumer.node,
                    "unsupported-private-target",
                    "own engine/bus cannot be an external consumer");
      make_link(a, &g->links[g->ns + g->no + i], out->published, consumer);
      out->consumer = consumer;
      out->bound = true;
      out->link_op = op;
      g->inventory = ++a->inventory;
      return;
    }
    if (kind == "stage") {
      keys(action, {"kind", "preparedToken", "expectedInventoryRevision"});
      require(!a->build.started && !g->stage_started &&
                  str(get(action, "preparedToken")) == g->prepared,
              "busy", "host build already started or stale prepared token");
      require(num(get(action, "expectedInventoryRevision")) ==
                  g->inventory.load(),
              "stale-inventory", "native inventory revision changed");
      require(all_bindings(g) && g->headers.load(std::memory_order_acquire),
              "clock-unqualified",
              "links/formats/fresh source headers not ready");
      g->clock = g->seen_clock.load(std::memory_order_acquire);
      require(g->clock &&
                  g->clock ==
                      g->generator_clock.load(std::memory_order_acquire) &&
                  graph_clock(g, g->clock),
              "clock-unqualified", "native common clock not observed");
      g->stage_op = op;
      g->install_op = op;
      g->admitted_inventory = g->inventory.load(std::memory_order_acquire);
      g->pending_target = a->control.target;
      a->ticket = g;
      g->stage_started = true;
      canonical_request(g);
      a->build.g = g;
      a->build.done.store(false);
      a->build.result = MX_INVALID;
      a->build.error[0] = 0;
      a->build.host = nullptr;
      require(!pthread_create(&a->build.thread, nullptr, builder, &a->build),
              "owner-failed", "host builder thread failed");
      a->build.started = true;
      return;
    }
    if (kind == "install") {
      keys(action, {"kind", "stagedToken"});
      require(a->control.present && a->control.consumed && !a->control.completed &&
                  a->control.action == MX_PRIVATE_STAGE &&
                  op == a->control.target.operation &&
                  g->staged_host && !g->staged.empty() &&
                  str(get(action, "stagedToken")) == g->staged &&
                  op == g->stage_op,
              "stale-generation", "staged token mismatch");
      require(!a->pending.load() && !g->switched.load() && all_bindings(g) &&
                  g->headers.load(std::memory_order_acquire) &&
                  g->inventory.load(std::memory_order_acquire) ==
                      g->admitted_inventory &&
                  g->seen_clock.load() == g->clock,
              "endpoint-changed", "native readiness changed before install");
      g->install_op = op;
      a->phase = "installing";
      a->ticket = g;
      // Release publishes one retained ticket to the exclusive callback.
      // No blocking SDK wait or new host allocation occurs at Install.
      a->pending.store(g, std::memory_order_release);
      return;
    }
    throw Error("protocol-error", "unknown companion action");
  } catch (const Error &e) {
    fault_reply(a, op, e.code, e.what());
  } catch (const std::bad_alloc &) {
    a->admission.store(false);
    a->failed_held = true;
    a->phase = "failed-held";
    fault_reply(a, op, "resource-limit", "allocation failed; owner retained");
  }
}

static void close_authority(App *a) {
  if (a->closing)
    return;
  a->closing = true;
  if (!a->fencing) {
    uint64_t epoch = a->epoch.load(std::memory_order_acquire);
    if (epoch >= UINT64_MAX - 1) {
      a->admission.store(false);
      a->failed_held = true;
      a->phase = "failed-held";
      return;
    }
    begin_fence(a, "control-eof", epoch + 1);
  }
}
static void pump(App *a, bool priority) {
  auto &input = priority ? a->cancel : a->normal;
  int fd = priority ? a->cancel_fd : STDIN_FILENO;
  if (input.closed)
    return;
  for (unsigned iteration = 0; iteration < 16; iteration++) {
    auto end = input.data.find('\n');
    if (end != std::string::npos) {
      std::string line = input.data.substr(0, end);
      input.data.erase(0, end + 1);
      if (!priority)
        pump(a, true); // priority cancellation cannot queue behind a mutator
      handle(a, line, priority);
      if (!priority)
        return;
      continue;
    }
    char buffer[4096];
    ssize_t n = read(fd, buffer, sizeof(buffer));
    if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
      return;
    if (n < 0 && errno == EINTR)
      continue;
    if (n <= 0) {
      input.closed = true;
      close_authority(a);
      return;
    }
    input.data.append(buffer, (size_t)n);
    if (input.data.size() > wire::limit) {
      input.closed = true;
      a->admission.store(false, std::memory_order_release);
      fault_reply(a, "invalid", "protocol-error",
                  "newline message exceeds 1 MiB");
      close_authority(a);
      return;
    }
  }
}
static void flush(App *a) {
  size_t remaining = 65536;
  while (!a->replies.empty() && remaining) {
    auto &line = a->replies.front();
    size_t n = std::min(remaining, line.size() - a->reply_offset);
    ssize_t written = write(STDOUT_FILENO, line.data() + a->reply_offset, n);
    if (written < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
      return;
    if (written < 0 && errno == EINTR)
      continue;
    if (written <= 0) {
      a->transport_failed = true;
      close_authority(a);
      return;
    }
    a->reply_offset += (size_t)written;
    a->queued_bytes -= (size_t)written;
    remaining -= (size_t)written;
    if (a->reply_offset == line.size()) {
      a->replies.pop_front();
      a->reply_offset = 0;
    }
  }
}
static void input_ready(void *data, int fd, uint32_t mask) {
  (void)fd;
  (void)mask;
  auto a = (App *)data;
  try {
    pump(a, true);
    pump(a, false);
  } catch (...) {
    a->admission.store(false);
    a->failed_held = true;
    a->phase = "failed-held";
  }
}
static void cancel_ready(void *data, int fd, uint32_t mask) {
  (void)fd;
  (void)mask;
  auto a = (App *)data;
  try {
    pump(a, true);
  } catch (...) {
    a->admission.store(false);
    a->failed_held = true;
    a->phase = "failed-held";
  }
}
static void tick(void *data, uint64_t expirations) {
  (void)expirations;
  auto a = (App *)data;
  try {
    pump(a, true);
    collect_builder(a);
    if (!a->ready && !a->fencing && !a->fault.load() && a->core_ok && a->modules == 63) {
      a->ready = true;
      auto e = event("ready");
      add(e, "inventoryRevision", a->inventory);
      add(e, "endpointSetDigest", digest("[]"));
      emit(a, "startup", e);
    }
    auto g = a->candidate;
    if (g && !a->fencing && !g->prepare_replied && !g->fault.load()) {
      if (link_internal(g)) {
        g->prepare_replied = true;
        a->phase = "prepared";
        auto e = event("prepared");
        add(e, "preparedToken", g->prepared);
        add(e, "inventoryRevision", g->inventory.load());
        add(e, "outputs", outputs_json(g));
        a->control.completed = true;
        emit(a, g->prepare_op, e);
      }
    }
    if (g && !a->fencing && g->prepare_replied) {
      for (size_t o = 0; o < g->no; o++) {
        auto &out = g->outputs[o];
        if (out.bound && !out.link_replied &&
            g->links[g->ns + g->no + o].active.load(
                std::memory_order_acquire) &&
            endpoint_exists(a, out.consumer, true, true) &&
            g->nodes[2 + o].out[0]->format_ok.load(std::memory_order_acquire)) {
          out.link_replied = true;
          auto e = event("linked");
          add(e, "inventoryRevision", g->inventory.load());
          add(e, "outputs", outputs_json(g));
          a->control.completed = true;
          emit(a, out.link_op, e);
        }
      }
    }
    if (g && !a->fencing && a->pending.load(std::memory_order_acquire) == g &&
        g->complete.load(std::memory_order_acquire) && !g->install_replied) {
      pump(a, true); // never acknowledge an install ahead of an already
                     // readable fence
      if (!a->fencing && current(g)) {
        g->install_replied = true;
        a->published_generation = g->generation;
        a->pending.store(nullptr, std::memory_order_release);
        auto e = event("installed");
        add(e, "receipt", installed(g));
        a->control.completed = true;
        emit(a, g->install_op, e);
        a->phase = "running";
      }
    }
    if (!a->fencing && !a->native_fault_reported &&
        (a->fault.load(std::memory_order_acquire) ||
         (g && g->fault.load(std::memory_order_acquire)))) {
      a->admission.store(false, std::memory_order_release);
      if (a->last_code.empty()) {
        a->last_code =
            g && g->fault.load() == 7 ? "source-lost" : "owner-failed";
        a->last_message =
            "private graph closed; native generation cannot be re-admitted";
      }
      fault_reply(
          a, a->pending.load() ? a->pending.load()->install_op : "native-fault",
          a->last_code, a->last_message);
      // Automatic loss closes/revokes; the parent still owns the exact
      // StopTarget and must Fence explicitly. It NEVER reopens admission.
      a->native_fault_reported = true;
      a->phase = "failed-held";
      if (pw_data_loop_invoke(a->owner, fault_owner, 0, nullptr, 0, false, a) <
          0)
        a->failed_held = true;
    }
    finish_fence(a);
    pump(a, false);
    flush(a);
    if (a->closing && a->fence_replied && a->replies.empty()) {
      a->exit_ready = true;
      pw_main_loop_quit(a->loop);
    }
  } catch (const Error &e) {
    a->admission.store(false, std::memory_order_release);
    a->failed_held = true;
    a->phase = "failed-held";
    a->last_code = e.code;
    a->last_message = e.what();
  } catch (...) {
    a->admission.store(false, std::memory_order_release);
    a->failed_held = true;
    a->phase = "failed-held";
  }
}

static void descriptor(App *a, const std::string &path,
                       const std::string &argv0) {
  require(absolute(path) == path, "protocol-error",
          "descriptor path must be canonical absolute");
  a->descriptor = wire::parse(read_file(path));
  auto d = a->descriptor.get();
  keys(d, {"schemaVersion", "scope", "sdkPrefix", "prefixManifest",
           "prefixManifestSha256", "companionExecutable", "companionSha256",
           "sdkVersion", "sdkCommit", "generators", "limits"});
  require(num(get(d, "schemaVersion")) == 1 &&
              str(get(d, "scope")) == "owned-private-generated-v1",
          "protocol-error", "unsupported descriptor scope/version");
  require(str(get(d, "sdkVersion")) == "1.4.8" &&
              str(get(d, "sdkCommit")) ==
                  "8b43ed5c910e892d53de5a96aa01e9280a5b6650",
          "unqualified-sdk",
          "only reviewed experimental 1.4.8 release-tree SDK is admitted");
  a->prefix = absolute(str(get(d, "sdkPrefix")));
  require(a->prefix == str(get(d, "sdkPrefix")), "unqualified-sdk",
          "SDK prefix must be canonical");
  require(absolute(str(get(d, "companionExecutable"))) == absolute(argv0) &&
              hex(str(get(d, "companionSha256"))),
          "unqualified-sdk", "companion identity mismatch");
  const auto manifest_path = str(get(d, "prefixManifest"));
  require(manifest_path[0] == '/' && hex(str(get(d, "prefixManifestSha256"))),
          "unqualified-sdk", "absolute qualified manifest required");
  auto manifest_bytes = read_file(manifest_path, 4 * wire::limit);
  require(digest(manifest_bytes) == str(get(d, "prefixManifestSha256")),
          "unqualified-sdk", "prefix manifest hash mismatch");
  auto manifest = wire::parse(manifest_bytes, nullptr, 4 * wire::limit);
  keys(manifest.get(), {"files", "symlinks", "totalBytes"});
  require(json_object_is_type(get(manifest.get(), "files"), json_type_object) &&
              json_object_is_type(get(manifest.get(), "symlinks"),
                                  json_type_object) &&
              num(get(manifest.get(), "totalBytes")) <= 384ull * 1024 * 1024,
          "resource-limit", "qualified prefix manifest budget");
  auto limits = get(d, "limits");
  keys(limits, {"maxSources", "maxOutputs", "maxResidentBytes", "maxFrames"});
  a->max_sources = num(get(limits, "maxSources"));
  a->max_outputs = num(get(limits, "maxOutputs"));
  a->max_bytes = num(get(limits, "maxResidentBytes"));
  require(a->max_sources && a->max_outputs && a->max_sources <= 4096 &&
              a->max_outputs <= 4096 &&
              num(get(limits, "maxFrames")) == frames &&
              a->max_bytes >= 8 * wire::limit &&
              a->max_bytes <= 256ull * 1024 * 1024,
          "resource-limit", "descriptor limits unsupported");
  a->resident = 6 * wire::limit;
  a->global_budget = 64 + 8 * (a->max_sources + a->max_outputs);
  a->globals.reserve(a->global_budget);
  auto generators = get(d, "generators");
  require(count(generators) > 0 && count(generators) <= a->max_sources,
          "resource-limit", "generator count outside budget");
  std::set<std::string> ids;
  for (size_t i = 0; i < count(generators); i++) {
    auto g = at(generators, i);
    keys(g, {"sourceId", "left", "right", "markerPeriod", "frameLimit"});
    require(ids.insert(str(get(g, "sourceId"))).second, "protocol-error",
            "duplicate generator sourceId");
    for (auto key : {"left", "right"}) {
      auto n = get(g, key);
      require(n &&
                  (json_object_is_type(n, json_type_double) ||
                   json_object_is_type(n, json_type_int)) &&
                  std::isfinite(json_object_get_double(n)),
              "protocol-error", "finite generated values required");
    }
    require(num(get(g, "markerPeriod")) <= UINT32_MAX, "protocol-error",
            "marker period overflow");
    (void)num(get(g, "frameLimit"));
  }
  const char *remote = getenv("PIPEWIRE_REMOTE");
  require(remote && !strcmp(remote, "mixyard-private"),
          "unsupported-private-target", "private remote required");
  private_dir(getenv("PIPEWIRE_RUNTIME_DIR"));
  private_dir(getenv("PIPEWIRE_CONFIG_DIR"));
  const char *config_name = getenv("PIPEWIRE_CONFIG_NAME");
  require(config_name && !strcmp(config_name, "client.conf"),
          "unsupported-private-target", "explicit client.conf required");
  std::string client_config =
      std::string(getenv("PIPEWIRE_CONFIG_DIR")) + "/client.conf";
  struct stat config_stat;
  require(!lstat(client_config.c_str(), &config_stat) &&
              S_ISREG(config_stat.st_mode) && config_stat.st_uid == getuid() &&
              (config_stat.st_mode & 0077) == 0,
          "unsupported-private-target",
          "owned private regular client config required");
  for (auto env :
       {"PIPEWIRE_MODULE_DIR", "SPA_PLUGIN_DIR", "LD_LIBRARY_PATH"}) {
    const char *p = getenv(env);
    require(p && strchr(p, ':') == nullptr && contained(absolute(p), a->prefix),
            "unqualified-sdk",
            "one explicit prefix library/module/plugin directory required");
  }
  require(!getenv("PIPEWIRE_CONFIG_PREFIX") && !getenv("PIPEWIRE_NO_CONFIG") &&
              !getenv("SPA_PLUGIN_PATH"),
          "unsupported-private-target",
          "unexpected native configuration override");
  Dl_info loaded{};
  require(dladdr((void *)(uintptr_t)&pw_init, &loaded) && loaded.dli_fname &&
              contained(absolute(loaded.dli_fname), a->prefix) &&
              !strcmp(pw_get_library_version(), "1.4.8"),
          "unqualified-sdk",
          "loaded PipeWire library is not the explicit reviewed 1.4.8 prefix");
  require(mx_host_abi_version() == MX_HOST_ABI_VERSION, "owner-failed",
          "canonical host ABI mismatch");
}
static void nonblocking(int fd) {
  int flags = fcntl(fd, F_GETFL);
  require(flags >= 0 && fcntl(fd, F_SETFL, flags | O_NONBLOCK) >= 0,
          "protocol-error", "control pipe cannot become nonblocking");
}
static void stop_signal(void *data, int signal) {
  (void)signal;
  close_authority((App *)data);
}
int main(int argc, char **argv) {
  App a;
  std::string descriptor_path;
  uint64_t epoch = 1;
  try {
    require(argc == 7 || argc == 9, "protocol-error",
            "usage: companion --descriptor ABS --instance-nonce HEX64 "
            "--cancel-fd FD [--control-epoch E]");
    std::set<std::string> seen;
    for (int i = 1; i < argc; i += 2) {
      std::string key = argv[i];
      require(seen.insert(key).second, "protocol-error", "duplicate argument");
      if (key == "--descriptor")
        descriptor_path = argv[i + 1];
      else if (key == "--instance-nonce")
        a.nonce = argv[i + 1];
      else if (key == "--cancel-fd") {
        uint64_t n = decimal(argv[i + 1]);
        require(n > 2 && n <= INT_MAX, "protocol-error", "cancel FD bound");
        a.cancel_fd = (int)n;
      } else if (key == "--control-epoch")
        epoch = decimal(argv[i + 1]);
      else
        throw Error("protocol-error", "unknown argument");
    }
    require(hex(a.nonce) && a.cancel_fd > 2 && epoch && epoch < UINT64_MAX - 1,
            "protocol-error", "nonce, epoch or cancel fd invalid");
    a.epoch.store(epoch);
    a.normal.data.reserve(wire::limit);
    a.cancel.data.reserve(wire::limit);
    require(mx_private_cancel_socket(a.cancel_fd, getppid()),
            "protocol-error", "inherited anonymous parent UnixStream socketpair required");
    nonblocking(STDIN_FILENO);
    nonblocking(STDOUT_FILENO);
    nonblocking(a.cancel_fd);
    signal(SIGPIPE, SIG_IGN);
    // Block before PipeWire/builder threads inherit their masks. Main-loop
    // signal ownership is independent of the priority cancellation pipe.
    sigset_t signals;
    sigemptyset(&signals);
    sigaddset(&signals, SIGTERM);
    sigaddset(&signals, SIGINT);
    require(!pthread_sigmask(SIG_BLOCK, &signals, nullptr), "owner-failed",
            "signal mask failed");
    descriptor(&a, descriptor_path, "/proc/self/exe");
    pw_init(nullptr, nullptr);
    a.loop = pw_main_loop_new(nullptr);
    require(a.loop, "owner-failed", "main loop unavailable");
    auto loop = pw_main_loop_get_loop(a.loop);
    a.context =
        pw_context_new(loop,
                       pw_properties_new("context.num-data-loops", "1",
                                         "config.name", "client.conf", nullptr),
                       0);
    require(a.context, "owner-failed", "private context unavailable");
    a.owner = pw_context_get_data_loop(a.context);
    require(a.owner, "owner-failed", "exclusive native DataLoop unavailable");
    a.core = pw_context_connect(
        a.context,
        pw_properties_new(PW_KEY_REMOTE_NAME, "mixyard-private", nullptr), 0);
    require(a.core, "unsupported-private-target",
            "explicit private core connection failed");
    pw_core_add_listener(a.core, &a.core_hook, &core_events, &a);
    a.registry = pw_core_get_registry(a.core, PW_VERSION_REGISTRY, 0);
    require(a.registry, "owner-failed", "registry unavailable");
    pw_registry_add_listener(a.registry, &a.registry_hook, &registry_events,
                             &a);
    require(pw_loop_add_io(loop, STDIN_FILENO,
                           SPA_IO_IN | SPA_IO_HUP | SPA_IO_ERR, false,
                           input_ready, &a) &&
                pw_loop_add_io(loop, a.cancel_fd,
                               SPA_IO_IN | SPA_IO_HUP | SPA_IO_ERR, false,
                               cancel_ready, &a),
            "owner-failed", "control IO registration failed");
    require(pw_loop_add_signal(loop, SIGTERM, stop_signal, &a) &&
                pw_loop_add_signal(loop, SIGINT, stop_signal, &a),
            "owner-failed", "signal registration failed");
    a.timer = pw_loop_add_timer(loop, tick, &a);
    require(a.timer, "owner-failed", "control completion timer unavailable");
    timespec interval{0, 5000000};
    require(pw_loop_update_timer(loop, a.timer, &interval, &interval, false) >=
                0,
            "owner-failed", "completion timer failed");
    pw_main_loop_run(a.loop);
    if (!a.exit_ready || !a.fence_replied || a.failed_held)
      return 3;
    // Native objects/pools have already been scrubbed and withdrawn through
    // the owner barriers. Context destruction retires the SDK itself before
    // the final Stopped event is serialized. All userdata stays alive here.
    pw_proxy_destroy((pw_proxy *)a.registry);
    a.registry = nullptr;
    pw_core_disconnect(a.core);
    a.core = nullptr;
    pw_context_destroy(a.context);
    a.context = nullptr;
    pw_main_loop_destroy(a.loop);
    a.loop = nullptr;
    pw_deinit();
    if (!a.shutdown_op.empty() && !a.transport_failed) {
      emit(&a, a.shutdown_op, event("stopped"));
      flush(&a);
      if (!a.replies.empty())
        return 3;
    }
    return 0;
  } catch (const Error &e) {
    fprintf(stderr, "private companion: %s: %s\n", e.code.c_str(), e.what());
    return 2;
  } catch (const std::exception &e) {
    fprintf(stderr, "private companion: %s\n", e.what());
    return 2;
  }
}
