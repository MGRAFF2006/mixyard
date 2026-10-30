/* SPDX-License-Identifier: MIT */
#include "owner.h"
#include "sha256.h"
#include <assert.h>
#include <stdio.h>

static void revision_and_clock(void) {
  struct mx_private_timeline t = {
      .clock_id = 19, .generation = 71, .revision = 28};
  mx_stamp stamp;
  assert(mx_private_stamp(&t, 19, 16384, 1000000000, &stamp));
  assert(stamp.generation == 71 && stamp.revision == 28 &&
         stamp.first_frame == 0 && stamp.sequence == 0);
  assert(mx_private_stamp(&t, 19, 16512, 1002666666, &stamp));
  assert(stamp.revision == 28 && stamp.first_frame == 128 &&
         stamp.sequence == 1);
  assert(!mx_private_stamp(&t, 19, 16768, 1005333333,
                           &stamp)); /* lost quantum is fatal */
  assert(!mx_private_stamp(&t, 20, 16640, 1005333333,
                           &stamp)); /* no clock substitution */
  assert(!mx_private_stamp(&t, 19, 16640, INT64_MAX, &stamp));
  struct mx_private_timeline r2 = {
      .clock_id = 83, .generation = 72, .revision = 29};
  assert(mx_private_stamp(&r2, 83, 96000, 3000000000, &stamp));
  assert(stamp.revision == 29 && stamp.first_frame == 0 &&
         stamp.clock_id == 83);
}
static void exact_pending_fence(void) {
  struct mx_private_stop_tuple expected = {.epoch = 3,
                                           .active = 71,
                                           .reserved = 72,
                                           .pending = true,
                                           .operation = "install-72"};
  struct mx_private_stop_tuple actual = expected;
  assert(mx_private_stop_equal(&expected, &actual));
  actual.active = 72;
  assert(!mx_private_stop_equal(&expected, &actual));
  actual = expected;
  actual.pending = false;
  assert(!mx_private_stop_equal(&expected, &actual));
  actual = expected;
  actual.operation[0] = 'x';
  assert(!mx_private_stop_equal(&expected, &actual));
  assert(mx_private_ticket_current(3, 3, true, 72, 72));
  assert(!mx_private_ticket_current(3, 3, false, 72,
                                    72)); /* close before owner barrier */
  assert(!mx_private_ticket_current(3, 4, true, 72,
                                    72)); /* a late ticket can never install */
  assert(!mx_private_ticket_current(3, 3, true, 71, 72));
  uint64_t last = 72;
  assert(!mx_private_reserve(&last, 72));
  assert(mx_private_reserve(&last, 74));
  assert(!mx_private_reserve(&last, 73));
  assert(!mx_private_reserve(&last, UINT64_MAX));
  struct mx_private_stop_tuple settled = {.epoch = 3, .active = 72};
  assert(mx_private_stop_accept(&expected, &expected, &expected, false));
  assert(mx_private_stop_accept(&expected, &settled, &expected, true));
  assert(mx_private_stop_accept(&settled, &settled, &expected, true));
  assert(!mx_private_stop_accept(&expected, &settled, &expected, false));
  struct mx_private_stop_tuple arbitrary = expected;
  arbitrary.active = 71 + 9;
  assert(!mx_private_stop_accept(&arbitrary, &settled, &expected, true));
  arbitrary = expected;
  arbitrary.epoch = 4;
  assert(!mx_private_stop_accept(&arbitrary, &settled, &expected, true));
}
static void digest_vectors(void) {
  char out[65];
  mx_sha_hex("", 0, out);
  assert(!strcmp(
      out, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
  mx_sha_hex("abc", 3, out);
  assert(!strcmp(
      out, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
  struct mx_sha256 s;
  mx_sha_init(&s);
  char block[1000];
  memset(block, 'a', sizeof(block));
  for (unsigned i = 0; i < 1000; i++)
    mx_sha_update(&s, block, sizeof(block));
  mx_sha_finish(&s, out);
  assert(!strcmp(
      out, "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"));
}
static void native_failure_classification(void) {
  double src[256], dest[256];
  for (unsigned i = 0; i < 256; i++)
    src[i] = 0.125;
  struct mx_private_input_view view = {.data = src,
                                       .n_datas = 1,
                                       .capacity = 2048,
                                       .size = 2048,
                                       .stride = 16,
                                       .pts = 1234567,
                                       .sequence = 128,
                                       .chunk = true,
                                       .header = true};
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_READY);
  assert(dest[255] == 0.125);
  view.size = 0;
  assert(mx_private_copy_input(&view, false, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_UNAVAILABLE);
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  for (unsigned i = 0; i < 256; i++)
    assert(dest[i] == 0.0);
  view.size = 2048;
  view.chunk_flags = 2;
  assert(mx_private_copy_input(&view, false, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_UNAVAILABLE);
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  view.chunk_flags = 1;
  assert(mx_private_copy_input(&view, false, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  view.chunk_flags = 0;
  view.size = 1024;
  assert(mx_private_copy_input(&view, false, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  view.size = 2048;
  view.header_flags = 1;
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  view.header_flags = 0;
  view.sequence = 0;
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  view.sequence = 128;
  view.pts++;
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
  view.pts--;
  view.n_datas = 2;
  assert(mx_private_copy_input(&view, true, 128, 1234567, 2, dest) ==
         MX_PRIVATE_INPUT_FATAL);
}
struct fake_native_token {
  unsigned returns;
  bool fail;
  bool canceled;
};
static bool fake_return(void *data) {
  struct fake_native_token *token = data;
  token->returns++;
  token->canceled = true; /* priority cancellation arrives during validation */
  return !token->fail;
}
static void every_borrow_returns(void) {
  double samples[256] = {0}, copy[256];
  struct mx_private_input_view view = {.data = samples,
                                       .n_datas = 1,
                                       .capacity = 2048,
                                       .size = 2048,
                                       .stride = 16,
                                       .pts = 1234,
                                       .sequence = 256,
                                       .chunk = true,
                                       .header = true};
  struct fake_native_token token = {0};
  assert(mx_private_copy_and_return(&view, true, 256, 1234, 2, copy,
                                    fake_return,
                                    &token) == MX_PRIVATE_INPUT_READY);
  assert(token.returns == 1 && token.canceled);
  view.size = 512;
  token.returns = 0;
  assert(mx_private_copy_and_return(&view, true, 256, 1234, 2, copy,
                                    fake_return,
                                    &token) == MX_PRIVATE_INPUT_FATAL);
  assert(token.returns == 1);
  view.size = 0;
  token.returns = 0;
  assert(mx_private_copy_and_return(&view, false, 256, 1234, 2, copy,
                                    fake_return,
                                    &token) == MX_PRIVATE_INPUT_UNAVAILABLE);
  assert(token.returns == 1);
  view.size = 2048;
  token.fail = true;
  token.returns = 0;
  samples[1] = 0.5;
  assert(mx_private_copy_and_return(&view, true, 256, 1234, 2, copy,
                                    fake_return,
                                    &token) == MX_PRIVATE_INPUT_FATAL);
  assert(token.returns == 1);
  for (unsigned i = 0; i < 256; i++)
    assert(copy[i] == 0.0);
}
int main(void) {
  revision_and_clock();
  exact_pending_fence();
  native_failure_classification();
  every_borrow_returns();
  digest_vectors();
  puts("owner clock/revision, exact fence tuple, late-ticket rejection, strict "
       "native input classification, SHA256: OK");
}
