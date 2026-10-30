/* SPDX-License-Identifier: MIT */
#ifndef MIXYARD_PRIVATE_OWNER_H
#define MIXYARD_PRIVATE_OWNER_H
#include "mixyard_host.h"
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

enum mx_private_input_result {
  MX_PRIVATE_INPUT_FATAL,
  MX_PRIVATE_INPUT_UNAVAILABLE,
  MX_PRIVATE_INPUT_READY
};
struct mx_private_input_view {
  const void *data;
  uint32_t n_datas, capacity, offset, size, chunk_flags, header_flags,
      header_offset;
  int32_t stride;
  int64_t pts, dts_offset;
  uint64_t sequence;
  bool chunk, header;
};
/* EMPTY has deliberately narrow meaning: an initialized, pre-admission native
 * allocation with no scheduled PCM. Once admitted, EMPTY/gap/partial/corrupt or
 * stale headers are fatal; they never become an implicit missing source. */
static inline enum mx_private_input_result
mx_private_copy_input(const struct mx_private_input_view *v, bool admitted,
                      uint64_t frame, uint64_t ns, uint32_t empty_flag,
                      double destination[256]) {
  memset(destination, 0, 2048);
  if (!v || !v->data || v->n_datas != 1 || v->capacity < 2048 || !v->chunk ||
      !v->header || v->offset)
    return MX_PRIVATE_INPUT_FATAL;
  if (!admitted &&
      ((v->size == 0 && (v->stride == 0 || v->stride == 16) &&
        (v->chunk_flags == 0 || v->chunk_flags == empty_flag)) ||
       (v->size == 2048 && v->stride == 16 && v->chunk_flags == empty_flag)))
    return MX_PRIVATE_INPUT_UNAVAILABLE;
  if (v->size != 2048 || v->stride != 16 || v->chunk_flags || v->header_flags ||
      v->header_offset || v->dts_offset || ns > INT64_MAX ||
      v->pts != (int64_t)ns || v->sequence != frame)
    return MX_PRIVATE_INPUT_FATAL;
  memcpy(destination, v->data, 2048);
  return MX_PRIVATE_INPUT_READY;
}

/* The native token is returned on every validation path in THIS call. The
 * callback adapter passes stack userdata; neither native tokens nor pointers
 * escape the borrow. A failed return clears the already copied owned bytes. */
static inline enum mx_private_input_result
mx_private_copy_and_return(const struct mx_private_input_view *view,
                           bool admitted, uint64_t frame, uint64_t ns,
                           uint32_t empty_flag, double destination[256],
                           bool (*return_borrow)(void *), void *userdata) {
  enum mx_private_input_result result =
      mx_private_copy_input(view, admitted, frame, ns, empty_flag, destination);
  if (!return_borrow(userdata)) {
    memset(destination, 0, 2048);
    return MX_PRIVATE_INPUT_FATAL;
  }
  return result;
}

/* Called only by the one native owner. The control epoch is checked again at
 * the switch and at every external commit. The priority fence closes admission
 * before it queues that owner's retirement barrier. */
struct mx_private_timeline {
  uint64_t clock_id, generation, revision, native_zero, next_native, next_frame,
      sequence;
  bool started;
};
static inline bool mx_private_stamp(struct mx_private_timeline *t,
                                    uint64_t clock, uint64_t frame, uint64_t ns,
                                    mx_stamp *stamp) {
  if (!clock || clock != t->clock_id || !t->generation || !t->revision ||
      frame > UINT64_MAX - 128 || ns > INT64_MAX - 100000000 ||
      t->sequence == UINT64_MAX || t->next_frame > UINT64_MAX - 128 ||
      (t->started && frame != t->next_native))
    return false;
  if (!t->started) {
    t->native_zero = frame;
    t->started = true;
  }
  *stamp = (mx_stamp){t->generation, t->revision, clock,         t->sequence++,
                      t->next_frame, ns,          ns + 100000000};
  t->next_frame += 128;
  t->next_native = frame + 128;
  return true;
}

/* Receipt coordinates stay native. The engine stamp remains rebased to zero. */
static inline bool
mx_private_processed_native(const struct mx_private_timeline *timeline,
                            const mx_stamp *stamp, uint64_t native_frame,
                            uint64_t *receipt_frame) {
  if (!timeline->started || native_frame < timeline->native_zero ||
      stamp->first_frame != native_frame - timeline->native_zero ||
      stamp->clock_id != timeline->clock_id ||
      stamp->generation != timeline->generation ||
      stamp->revision != timeline->revision)
    return false;
  *receipt_frame = native_frame;
  return true;
}

enum mx_private_format_fault {
  MX_PRIVATE_FORMAT_NONE,
  MX_PRIVATE_FORMAT_SOURCE,
  MX_PRIVATE_FORMAT_DESTINATION
};
/* Called by the production Format handler. Generator outputs and engine
 * inputs are source authority; engine outputs and bus ports are destinations.
 */
static inline enum mx_private_format_fault
mx_private_format_scope(unsigned role, bool output, bool exact, bool present,
                        bool admitted) {
  if (exact || (!present && !admitted))
    return MX_PRIVATE_FORMAT_NONE;
  return role == 2 || (role == 1 && output) ? MX_PRIVATE_FORMAT_DESTINATION
                                            : MX_PRIVATE_FORMAT_SOURCE;
}

struct mx_private_stop_tuple {
  uint64_t epoch, active, reserved;
  bool pending;
  char operation[65];
};
static inline bool
mx_private_stop_equal(const struct mx_private_stop_tuple *a,
                      const struct mx_private_stop_tuple *b) {
  return a->epoch == b->epoch && a->active == b->active &&
         a->pending == b->pending &&
         (!a->pending ||
          (a->reserved == b->reserved && !strcmp(a->operation, b->operation)));
}
/* One ticket has exactly two coordinator views: its retained Stage reservation
 * and, after a receipt is emitted, its settled generation. Physical callback
 * progress cannot invalidate an exact pending Stop already sent by control. */
static inline bool
mx_private_stop_accept(const struct mx_private_stop_tuple *expected,
                       const struct mx_private_stop_tuple *current,
                       const struct mx_private_stop_tuple *retained,
                       bool receipt_emitted) {
  return mx_private_stop_equal(expected, current) ||
         (receipt_emitted && retained &&
          mx_private_stop_equal(expected, retained));
}
enum mx_private_action {
  MX_PRIVATE_PREPARE = 1,
  MX_PRIVATE_LINK,
  MX_PRIVATE_STAGE
};
struct mx_private_reservation {
  struct mx_private_stop_tuple target;
  enum mx_private_action action;
  char digest[65];
  bool present, consumed, completed, fenced;
};
/* This is an actual received priority reservation, not a Fence echo. One live
 * command and its exact completed control view are retained until next reserve.
 * The normal frame cannot mutate anything until its digest is verified. */
static inline bool
mx_private_control_reserve(struct mx_private_reservation *r,
                           const struct mx_private_stop_tuple *target,
                           enum mx_private_action action, const char *digest) {
  if (r->fenced || (r->present && !r->completed) || !target->pending ||
      !target->epoch || !target->reserved || !target->operation[0] ||
      action < MX_PRIVATE_PREPARE || action > MX_PRIVATE_STAGE ||
      strlen(digest) != 64)
    return false;
  for (unsigned i = 0; i < 64; i++)
    if (!((digest[i] >= '0' && digest[i] <= '9') ||
          (digest[i] >= 'a' && digest[i] <= 'f')))
      return false;
  r->target = *target;
  r->action = action;
  memcpy(r->digest, digest, 65);
  r->present = true;
  r->consumed = false;
  r->completed = false;
  return true;
}
static inline bool
mx_private_control_matches(const struct mx_private_reservation *r,
                           uint64_t epoch, const char *operation,
                           enum mx_private_action action, const char *digest) {
  return r->present && r->target.epoch == epoch && r->action == action &&
         !strcmp(r->target.operation, operation) && !strcmp(r->digest, digest);
}
static inline bool mx_private_control_consume(struct mx_private_reservation *r,
                                              uint64_t epoch,
                                              const char *operation,
                                              enum mx_private_action action,
                                              const char *digest) {
  if (r->fenced || r->consumed ||
      !mx_private_control_matches(r, epoch, operation, action, digest))
    return false;
  r->consumed = true;
  return true;
}
static inline bool
mx_private_control_fence(struct mx_private_reservation *r,
                         const struct mx_private_stop_tuple *expected,
                         const struct mx_private_stop_tuple *current) {
  if (r->fenced ||
      !mx_private_stop_accept(expected, current, r->present ? &r->target : NULL,
                              r->completed))
    return false;
  r->fenced = true;
  return true;
}
/* No wraparound and no reuse of a generation, including canceled reservations.
 */
static inline bool mx_private_reserve(uint64_t *last, uint64_t next) {
  if (!next || next <= *last || next == UINT64_MAX)
    return false;
  *last = next;
  return true;
}
static inline bool mx_private_ticket_current(uint64_t ticket_epoch,
                                             uint64_t epoch, bool admission,
                                             uint64_t ticket_generation,
                                             uint64_t reserved) {
  return admission && epoch && ticket_epoch == epoch && ticket_generation &&
         ticket_generation == reserved;
}
#endif
